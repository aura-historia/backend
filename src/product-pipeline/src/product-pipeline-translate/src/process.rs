use crate::adapter::TranslationAdapter;
use common::{
    batch::Batch,
    language::{domain::Language, record::LanguageRecord},
    product_id::ProductId,
};
use itertools::{Chunk, Itertools};
use product_pipeline_common::{
    process::{PipeProcessor, ProcessResult},
    types::{CleansedPipeProduct, TranslatedPipeProduct},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    vec::IntoIter,
};
use strum::{EnumCount, IntoEnumIterator};
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

impl PipeProcessor<CleansedPipeProduct, TranslatedPipeProduct> for TranslationPipeProcesserImpl {
    fn process(&self, ins: Vec<CleansedPipeProduct>) -> ProcessResult<TranslatedPipeProduct> {
        let count = ins.len();
        let mut out_products = ins
            .into_iter()
            .map(|in_product| {
                let out_product = TranslatedPipeProduct {
                    product_id: in_product.product_id,
                    shop_id: in_product.shop_id,
                    shops_product_id: in_product.shops_product_id,
                    native_title: in_product.native_title,
                    other_title: HashMap::with_capacity(Language::COUNT),
                    native_description: in_product.native_description,
                    other_description: HashMap::with_capacity(Language::COUNT),
                };
                (out_product.product_id, out_product)
            })
            .collect::<HashMap<_, _>>();
        let mut failures = HashSet::new();

        let mut all_titles: HashMap<LanguageRecord, Vec<(ProductId, String)>> =
            HashMap::with_capacity(out_products.len());
        let mut all_descriptions: HashMap<LanguageRecord, Vec<(ProductId, String)>> =
            HashMap::with_capacity(out_products.len());
        for out_product in out_products.values() {
            all_titles
                .entry(out_product.native_title.language)
                .or_default()
                .push((
                    out_product.product_id,
                    out_product.native_title.text.clone(),
                ));
            if let Some(ref native_description) = out_product.native_description {
                all_descriptions
                    .entry(native_description.language)
                    .or_default()
                    .push((out_product.product_id, native_description.text.clone()));
            }
        }

        for (lang, product_ids_native_titles) in all_titles {
            let chunks = product_ids_native_titles
                .into_iter()
                .chunks(TITLE_BATCH_SIZE);
            for titles_chunk in chunks.into_iter() {
                let chunk_failures = self.handle_translation_chunk(
                    titles_chunk,
                    &lang.into(),
                    &mut out_products,
                    |out_product, tgt_lang, translation| {
                        out_product.other_title.insert(tgt_lang, translation);
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
                    &lang.into(),
                    &mut out_products,
                    |out_product, tgt_lang, translation| {
                        out_product.other_description.insert(tgt_lang, translation);
                    },
                );
                failures.extend(chunk_failures);
            }
        }

        out_products.retain(|product_id, _| !failures.contains(product_id));

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Translated cleansed products."
        );

        ProcessResult {
            successes: out_products.into_values().collect(),
            failures,
        }
    }
}

impl TranslationPipeProcesserImpl {
    fn handle_translation_chunk(
        &self,
        chunk: Chunk<'_, IntoIter<(ProductId, String)>>,
        src_lang: &Language,
        out_products: &mut HashMap<ProductId, TranslatedPipeProduct>,
        apply_translation: impl Fn(&mut TranslatedPipeProduct, LanguageRecord, String),
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
        let tgt_langs = Language::iter().filter(|tgt| tgt != src_lang);
        for tgt_lang in tgt_langs {
            match self
                .translation_delegate
                .translate_batch(&title_batch, src_lang, &tgt_lang)
            {
                Ok(translated) => {
                    let translateds = product_ids_chunk
                        .iter()
                        .zip(translated.into_iter())
                        .collect::<HashMap<_, _>>();
                    for (product_id, translated) in translateds {
                        if let Some(out_product) = out_products.get_mut(product_id) {
                            apply_translation(out_product, tgt_lang.into(), translated);
                        } else {
                            error!(productId = %product_id, "Expected to find PipeProduct but didn't.");
                        }
                    }
                }
                Err(err) => {
                    error!(error = %err, srcLang = src_lang.as_str(), tgtLang = tgt_lang.as_str(), "Failed translation.");
                    failures.extend(product_ids_chunk.into_iter());
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
    use common::language::record::{LanguageRecord, TextRecord};
    use common::product_id::ProductId;
    use fake::rand::seq::SliceRandom;
    use fake::{Fake, Faker};
    use product_pipeline_common::process::PipeProcessor;
    use product_pipeline_common::types::CleansedPipeProduct;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use rstest;
    use std::sync::Arc;

    #[test]
    fn should_enrich_titles_for_other_langs() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let in_products = fake::vec![CleansedPipeProduct; 1];
        let actuals = translation_pipe_processor.process(in_products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual = actuals.successes[0].clone();
        assert!(!actual.other_title.is_empty());
        assert!(actual.other_title.iter().all(|(_, title)| title == "Foo"));
    }

    #[test]
    fn should_not_enrich_title_for_id_lang() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let in_product: CleansedPipeProduct = Faker.fake();
        let in_products = vec![in_product.clone()];
        let actuals = translation_pipe_processor.process(in_products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual = actuals.successes[0].clone();
        assert!(
            !actual
                .other_title
                .iter()
                .any(|(other_lang, _)| other_lang == &in_product.native_title.language)
        );
    }

    #[test]
    fn should_enrich_description_for_other_langs() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let mut in_product: CleansedPipeProduct = Faker.fake();
        in_product.native_description = Some(Faker.fake());
        let in_products = vec![in_product];
        let actuals = translation_pipe_processor.process(in_products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual = actuals.successes[0].clone();
        assert!(!actual.other_description.is_empty());
        assert!(
            actual
                .other_description
                .iter()
                .all(|(_, title)| title == "Foo")
        );
    }

    #[test]
    fn should_not_enrich_description_for_id_lang() {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));

        let mut in_product: CleansedPipeProduct = Faker.fake();
        in_product.native_description = Some(Faker.fake());
        let in_products = vec![in_product.clone()];
        let actuals = translation_pipe_processor.process(in_products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual = actuals.successes[0].clone();
        assert!(
            !actual
                .other_description
                .iter()
                .any(|(other_lang, _)| other_lang
                    == &in_product.native_description.clone().unwrap().language)
        );
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
    fn should_process_translation(#[case] count: usize) {
        let mut adapter = MockTranslationAdapter::default();
        adapter
            .expect_translate_batch()
            .returning(|batch, _, _| Ok(batch.clone()));
        let translation_pipe_processor = TranslationPipeProcesserImpl::new(Arc::new(adapter));

        let product_id = ProductId::new();
        let mut products = fake::vec![CleansedPipeProduct; count];
        let product = CleansedPipeProduct {
            product_id,
            shop_id: Faker.fake(),
            shops_product_id: Faker.fake(),
            native_title: TextRecord {
                text: "blue".to_owned(),
                language: LanguageRecord::En,
            },
            native_description: Some(TextRecord {
                text: "Hallo Welt!".to_owned(),
                language: LanguageRecord::De,
            }),
        };
        products.push(product);
        products.shuffle(&mut fake::rand::rng());

        let actual = translation_pipe_processor.process(products);
        assert!(actual.failures.is_empty());
        assert_eq!(count + 1, actual.successes.len());

        let expected = actual
            .successes
            .into_iter()
            .find(|out_product| out_product.product_id == product_id)
            .unwrap();

        assert_eq!(LanguageRecord::En, expected.native_title.language);
        assert_eq!("blue", &expected.native_title.text);
        assert!(expected.other_title.contains_key(&LanguageRecord::De));
        assert!(expected.other_title.contains_key(&LanguageRecord::Fr));
        assert!(expected.other_title.contains_key(&LanguageRecord::Es));
        assert_eq!(
            LanguageRecord::De,
            expected.native_description.as_ref().unwrap().language
        );
        assert_eq!(
            "Hallo Welt!",
            expected.native_description.as_ref().unwrap().text
        );
        assert!(expected.other_description.contains_key(&LanguageRecord::En));
        assert!(expected.other_description.contains_key(&LanguageRecord::Fr));
        assert!(expected.other_description.contains_key(&LanguageRecord::Es));
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
    fn should_partially_fail(#[case] count: usize) {
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
        let mut products = fake::vec![CleansedPipeProduct; count];

        // we need to force the values of each title/description to have the same language
        // due to the grouping into a HashMap<Language, _> which varies batch_len
        // together with the expectation this now only ever fails the very last non-full batch
        for in_product in &mut products {
            in_product.native_title = TextRecord {
                language: LanguageRecord::Es,
                text: Faker.fake(),
            };
            in_product.native_description = Some(TextRecord {
                language: LanguageRecord::De,
                text: Faker.fake(),
            });
        }

        let actual = translation_pipe_processor.process(products);

        assert_eq!(count - (count % 32), actual.successes.len());
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
    fn should_partially_fail_all(#[case] count: usize) {
        let mut translation_delegate = MockTranslationAdapter::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let translation_pipe_processor =
            TranslationPipeProcesserImpl::new(Arc::new(translation_delegate));
        let actual = translation_pipe_processor.process(fake::vec![CleansedPipeProduct; count]);

        assert!(actual.successes.is_empty());
        assert_eq!(count, actual.failures.len());
    }
}
