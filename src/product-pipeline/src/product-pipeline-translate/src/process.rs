use crate::adapter::TranslationAdapter;
use common::language::domain::Language;
use common::{batch::Batch, product_id::ProductId};
use itertools::{Chunk, Itertools};
use product::core::{product::Product, product_event::ProductEventPayload};
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    vec::IntoIter,
};
use strum::IntoEnumIterator;
use tracing::{error, info};

pub struct TranslationPipeProcesserImpl {
    translation_delegate: Arc<dyn TranslationAdapter + Send + Sync>,
}

impl TranslationPipeProcesserImpl {
    pub fn new(translation_delegate: Arc<dyn TranslationAdapter + Send + Sync>) -> Self {
        Self {
            translation_delegate,
        }
    }
}

// Must both be <= 64 to fit into Batch<64,_>
const TITLE_BATCH_SIZE: usize = 32;
const DESCRIPTION_BATCH_SIZE: usize = 8;

#[async_trait::async_trait]
impl PipeProcessor for TranslationPipeProcesserImpl {
    async fn process(&self, ins: Vec<Product>) -> ProcessResult {
        let count = ins.len();
        let mut products = ins
            .into_iter()
            .map(|product| (product.product_id, product))
            .collect::<HashMap<_, _>>();
        let mut successes = Vec::with_capacity(4 * count);
        let mut failures = HashSet::new();

        let mut all_titles: HashMap<Language, Vec<(ProductId, String)>> =
            HashMap::with_capacity(products.len());
        let mut all_descriptions: HashMap<Language, Vec<(ProductId, String)>> =
            HashMap::with_capacity(products.len());

        // Sort products by text length so each batch contains items of similar length,
        // reducing padding overhead in transformer models. Using a consistent sort order
        // for both titles and descriptions ensures batches overlap, so a product that
        // fails in a title batch is also in the corresponding description batch.
        let mut sorted_products: Vec<&Product> = products.values().collect();
        sorted_products.sort_by_key(|product| {
            product.native_title.payload.len()
                + product
                    .native_description
                    .as_ref()
                    .map_or(0, |d| d.payload.len())
        });

        for product in sorted_products {
            all_titles
                .entry(product.native_title.localization)
                .or_default()
                .push((product.product_id, product.native_title.payload.to_string()));
            if let Some(ref native_description) = product.native_description {
                all_descriptions
                    .entry(native_description.localization)
                    .or_default()
                    .push((product.product_id, native_description.payload.to_string()));
            }
        }

        for (lang, product_ids_native_titles) in all_titles {
            let chunks = product_ids_native_titles
                .into_iter()
                .chunks(TITLE_BATCH_SIZE);
            for titles_chunk in chunks.into_iter() {
                let chunk_failures = self.handle_translation_chunk(
                    titles_chunk,
                    &lang,
                    &mut products,
                    |product, tgt_lang, translation| {
                        if let Some(translation_event) =
                            product.translate_title(lang, tgt_lang, translation.into())
                        {
                            successes
                                .push(translation_event.map_payload(ProductEventPayload::from));
                        }
                    },
                );
                failures.extend(chunk_failures);
            }
        }

        for (lang, product_ids_native_descriptions) in all_descriptions {
            let chunks = product_ids_native_descriptions
                .into_iter()
                .chunks(DESCRIPTION_BATCH_SIZE);
            for descriptions_chunk in chunks.into_iter() {
                let chunk_failures = self.handle_translation_chunk(
                    descriptions_chunk,
                    &lang,
                    &mut products,
                    |product, tgt_lang, translation| {
                        if let Some(translation_event) =
                            product.translate_description(lang, tgt_lang, translation.into())
                        {
                            successes
                                .push(translation_event.map_payload(ProductEventPayload::from));
                        }
                    },
                );
                failures.extend(chunk_failures);
            }
        }

        products.retain(|product_id, _| !failures.contains(product_id));

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Translated cleansed products."
        );

        ProcessResult {
            successes,
            failures,
        }
    }
}

impl TranslationPipeProcesserImpl {
    fn handle_translation_chunk(
        &self,
        chunk: Chunk<'_, IntoIter<(ProductId, String)>>,
        src_lang: &Language,
        products: &mut HashMap<ProductId, Product>,
        mut apply_translation: impl FnMut(&mut Product, Language, String),
    ) -> HashSet<ProductId> {
        let mut failures = HashSet::new();

        // these need to be Vec because we zip later!
        let (product_ids_chunk, native_titles_chunk): (Vec<_>, Vec<_>) = chunk.into_iter().unzip();
        let title_batch: Batch<String, 64> = Batch::try_from_iter(
            native_titles_chunk
                .into_iter()
                .map(|title| title.to_string()),
        )
        .expect(
            "shouldn't fail creating Batch of size 64 because 'itertools::chunks(64)' and 'Batch'
                share the same semantics being invoked with same size",
        );
        let tgt_langs =
            Language::iter().filter(|tgt| tgt != src_lang && tgt.is_translation_target());
        for tgt_lang in tgt_langs {
            match self
                .translation_delegate
                .translate_batch(&title_batch, src_lang, &tgt_lang)
            {
                Ok(translated) => {
                    let translateds = product_ids_chunk
                        .iter()
                        .zip(translated)
                        .collect::<HashMap<_, _>>();
                    for (product_id, translated) in translateds {
                        if let Some(product) = products.get_mut(product_id) {
                            apply_translation(product, tgt_lang, translated);
                        } else {
                            error!(productId = %product_id, "Expected to find PipeProduct but didn't.");
                        }
                    }
                }
                Err(err) => {
                    error!(error = %err, srcLang = src_lang.as_str(), tgtLang = tgt_lang.as_str(), "Failed translation.");
                    failures.extend(product_ids_chunk);
                    break;
                }
            }
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use crate::process::{DESCRIPTION_BATCH_SIZE, TITLE_BATCH_SIZE};
    use crate::{adapter::MockTranslationAdapter, process::TranslationPipeProcesserImpl};
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::product_id::ProductId;
    use fake::rand::seq::SliceRandom;
    use fake::{Fake, Faker};
    use product::core::product::Product;
    use product::core::product_event::ProductEventPayload;
    use product::core::product_event::enrichment::ProductEnrichmentEventPayload;
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[tokio::test]
    async fn should_enrich_titles_for_other_langs() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let products = fake::vec![Product; 1];
        let actuals = translation_pipe_processor.process(products).await;

        assert!(actuals.failures.is_empty());

        let actual = actuals.successes;
        assert!(!actual.is_empty());
        assert!(actual.iter().all(|event| match event.payload.clone() {
            ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::TranslatedTitle(payload),
            ) => {
                "Foo" == payload.target.to_string()
            }
            ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::TranslatedDescription(payload),
            ) => {
                "Foo" == payload.target.to_string()
            }
            _ => true,
        }));
    }

    #[tokio::test]
    async fn should_not_enrich_title_for_id_lang() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let product: Product = Faker.fake();
        let products = vec![product.clone()];
        let actuals = translation_pipe_processor.process(products).await;

        assert!(actuals.failures.is_empty());

        let actual = actuals.successes;
        assert!(
            actual
                .iter()
                .find_map(|event| match event.payload.clone() {
                    ProductEventPayload::ProductEnrichmentEvent(
                        ProductEnrichmentEventPayload::TranslatedTitle(payload),
                    ) => {
                        if payload.target_language == product.native_title.localization {
                            Some(())
                        } else {
                            None
                        }
                    }
                    ProductEventPayload::ProductEnrichmentEvent(
                        ProductEnrichmentEventPayload::TranslatedDescription(payload),
                    ) => {
                        if payload.target_language
                            == product.native_description.as_ref().unwrap().localization
                        {
                            Some(())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn should_enrich_description_for_other_langs() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let mut product: Product = Faker.fake();
        product.native_description = Some(Faker.fake());
        let products = vec![product];
        let actuals = translation_pipe_processor.process(products).await;

        assert!(actuals.failures.is_empty());

        let actual = actuals.successes;
        assert!(!actual.is_empty());
        assert!(actual.iter().all(|event| match event.payload.clone() {
            ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::TranslatedTitle(payload),
            ) => {
                "Foo" == payload.target.to_string()
            }
            ProductEventPayload::ProductEnrichmentEvent(
                ProductEnrichmentEventPayload::TranslatedDescription(payload),
            ) => {
                "Foo" == payload.target.to_string()
            }
            _ => true,
        }));
    }

    #[tokio::test]
    async fn should_not_enrich_description_for_id_lang() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let mut product: Product = Faker.fake();
        product.native_description = Some(Faker.fake());
        let products = vec![product.clone()];
        let actuals = translation_pipe_processor.process(products).await;

        assert!(actuals.failures.is_empty());

        let actual = actuals.successes;
        assert!(
            actual
                .iter()
                .find_map(|event| match event.payload.clone() {
                    ProductEventPayload::ProductEnrichmentEvent(
                        ProductEnrichmentEventPayload::TranslatedTitle(payload),
                    ) => {
                        if payload.target_language == product.native_title.localization {
                            Some(())
                        } else {
                            None
                        }
                    }
                    ProductEventPayload::ProductEnrichmentEvent(
                        ProductEnrichmentEventPayload::TranslatedDescription(payload),
                    ) => {
                        if payload.target_language
                            == product.native_description.as_ref().unwrap().localization
                        {
                            Some(())
                        } else {
                            None
                        }
                    }
                    _ => None,
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn should_not_translate_into_ingestion_only_langs() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["翻訳".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        // Use an ingestion-only language as the source
        let mut product: Product = Faker.fake();
        product.native_title = Localized {
            payload: "这是一个中文标题".into(),
            localization: Language::Zh,
        };
        product.native_description = None;
        let actuals = translation_pipe_processor.process(vec![product]).await;

        assert!(actuals.failures.is_empty());

        let target_languages: HashSet<_> = actuals
            .successes
            .iter()
            .filter_map(|event| match event.payload.clone() {
                ProductEventPayload::ProductEnrichmentEvent(
                    ProductEnrichmentEventPayload::TranslatedTitle(payload),
                ) => Some(payload.target_language),
                _ => None,
            })
            .collect();

        // Must only translate into fully-supported languages
        assert!(target_languages.contains(&Language::De));
        assert!(target_languages.contains(&Language::En));
        assert!(target_languages.contains(&Language::Fr));
        assert!(target_languages.contains(&Language::Es));
        assert!(target_languages.contains(&Language::It));

        // Must NOT translate into ingestion-only languages
        assert!(!target_languages.contains(&Language::Zh));
        assert!(!target_languages.contains(&Language::Pt));
        assert!(!target_languages.contains(&Language::Pl));
        assert!(!target_languages.contains(&Language::Tr));
        assert!(!target_languages.contains(&Language::Nl));
        assert!(!target_languages.contains(&Language::Cs));
        assert!(!target_languages.contains(&Language::Ja));
        assert!(!target_languages.contains(&Language::Ru));
        assert!(!target_languages.contains(&Language::Ar));
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(7)]
    #[case(15)]
    #[case(16)]
    #[case(17)]
    #[case(21)]
    #[case(31)]
    #[case(42)]
    #[case(63)]
    #[case(64)]
    #[case(65)]
    #[case(69)]
    #[case(144)]
    #[tokio::test]
    async fn should_process_translation(#[case] count: usize) {
        let mut adapter = MockTranslationAdapter::default();
        adapter
            .expect_translate_batch()
            .returning(|batch, _, _| Ok(batch.clone()));
        let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

        let product_id = ProductId::new();
        let mut products = fake::vec![Product; count];
        let mut product = Faker.fake::<Product>();
        product.product_id = product_id;
        product.native_title = Localized {
            payload: "blue".into(),
            localization: Language::En,
        };
        product.native_description = Some(Localized {
            payload: "Hallo Welt!".into(),
            localization: Language::De,
        });
        products.push(product);
        products.shuffle(&mut fake::rand::rng());

        let actual = translation_pipe_processor.process(products).await;
        assert!(actual.failures.is_empty());

        let title_languages = actual
            .successes
            .iter()
            .filter(|event| event.aggregate_id == product_id)
            .filter_map(|event| {
                event
                    .payload
                    .as_enrichment_event()
                    .unwrap()
                    .as_translated_title()
            })
            .map(|payload| payload.target_language)
            .collect::<HashSet<_>>();
        assert_eq!(4, title_languages.len());
        assert!(title_languages.contains(&Language::De));
        assert!(title_languages.contains(&Language::Fr));
        assert!(title_languages.contains(&Language::Es));
        assert!(title_languages.contains(&Language::It));

        let description_languages = actual
            .successes
            .iter()
            .filter(|event| event.aggregate_id == product_id)
            .filter_map(|event| {
                event
                    .payload
                    .as_enrichment_event()
                    .unwrap()
                    .as_translated_description()
            })
            .map(|payload| payload.target_language)
            .collect::<HashSet<_>>();
        assert!(description_languages.contains(&Language::Fr));
        assert!(description_languages.contains(&Language::Es));
        assert!(description_languages.contains(&Language::It));
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(42)]
    #[case(64)]
    #[case(69)]
    #[case(128)]
    #[case(141)]
    #[case(500)]
    #[case(10001)]
    #[tokio::test]
    async fn should_partially_fail(#[case] count: usize) {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|batch, _, _| {
                if batch.len() == TITLE_BATCH_SIZE || batch.len() == DESCRIPTION_BATCH_SIZE {
                    Ok(fake::vec![String; 32].try_into().unwrap())
                } else {
                    Err(PyErr::new::<PyTypeError, _>("Something went wrong"))
                }
            });

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));
        let mut products = fake::vec![Product; count];

        // we need to force the values of each title/description to have the same language
        // due to the grouping into a HashMap<Language, _> which varies batch_len
        // together with the expectation this now only ever fails the very last non-full batch
        for product in &mut products {
            product.native_title = Localized {
                localization: Language::Es,
                payload: Faker.fake(),
            };
            product.native_description = Some(Localized {
                localization: Language::De,
                payload: Faker.fake(),
            });
        }

        let actual = translation_pipe_processor.process(products).await;

        assert_eq!(count % 32, actual.failures.len());
    }

    #[rstest::rstest]
    #[trace]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(42)]
    #[case(500)]
    #[case(1000)]
    #[tokio::test]
    async fn should_partially_fail_all(#[case] count: usize) {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));
        let actual = translation_pipe_processor
            .process(fake::vec![Product; count])
            .await;

        assert!(actual.successes.is_empty());
        assert_eq!(count, actual.failures.len());
    }
}
