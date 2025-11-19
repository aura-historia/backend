use crate::{
    pipeline::pipe::{EnrichmentPipe, PipeProduct, PipeResult},
    translate::TranslationDelegate,
};
use common::{batch::Batch, language::domain::Language, product_id::ProductId};
use itertools::{Chunk, Itertools};
use product::core::{description::Description, title::Title};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    vec::IntoIter,
};
use strum::IntoEnumIterator;
use tracing::{error, info};

pub struct TranslationEnrichmentPipeImpl {
    translation_delegate: Arc<dyn TranslationDelegate + Send + Sync>,
}

impl TranslationEnrichmentPipeImpl {
    pub fn new(translation_delegate: Arc<dyn TranslationDelegate + Send + Sync>) -> Self {
        Self {
            translation_delegate,
        }
    }
}

// Most both be <= 64 to fit into Batch<64,_>
const TITLE_BATCH_SIZE: usize = 64;
const DESCRIPTION_BATCH_SIZE: usize = 16;

impl EnrichmentPipe for TranslationEnrichmentPipeImpl {
    fn enrich(&self, products: Vec<PipeProduct>) -> PipeResult {
        let count = products.len();
        let mut products = products
            .into_iter()
            .map(|product| (product.source.product_id, product))
            .collect::<HashMap<_, _>>();
        let mut failures = HashSet::new();

        let mut all_titles: HashMap<Language, Vec<(ProductId, Title)>> =
            HashMap::with_capacity(products.len());
        let mut all_descriptions: HashMap<Language, Vec<(ProductId, Description)>> =
            HashMap::with_capacity(products.len());
        for product in products.values() {
            all_titles
                .entry(product.source.payload.native_title.localization)
                .or_default()
                .push((
                    product.source.product_id,
                    product.source.payload.native_title.payload.clone(),
                ));
            if let Some(ref native_description) = product.source.payload.native_description {
                all_descriptions
                    .entry(native_description.localization)
                    .or_default()
                    .push((
                        product.source.product_id,
                        native_description.payload.clone(),
                    ));
            }
        }

        for (lang, product_ids_native_titles) in all_titles {
            let chunks = product_ids_native_titles
                .into_iter()
                .chunks(TITLE_BATCH_SIZE);
            for titles_chunk in chunks.into_iter() {
                let chunk_failures =
                    self.handle_translation_chunk_titles(titles_chunk, &lang, &mut products);
                failures.extend(chunk_failures);
            }
        }

        for (lang, product_ids_native_descriptions) in all_descriptions {
            let chunks = product_ids_native_descriptions
                .into_iter()
                .chunks(DESCRIPTION_BATCH_SIZE);
            for descriptions_chunk in chunks.into_iter() {
                let chunk_failures = self.handle_translation_chunk_descriptions(
                    descriptions_chunk,
                    &lang,
                    &mut products,
                );
                failures.extend(chunk_failures);
            }
        }

        products.retain(|product_id, _| !failures.contains(product_id));

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Translated PipeProducts."
        );

        PipeResult {
            successes: products.into_values().collect(),
            failures,
        }
    }
}

impl TranslationEnrichmentPipeImpl {
    fn handle_translation_chunk_titles(
        &self,
        chunk: Chunk<'_, IntoIter<(ProductId, Title)>>,
        src_lang: &Language,
        products: &mut HashMap<ProductId, PipeProduct>,
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
                        if let Some(pipe_product) = products.get_mut(product_id) {
                            match tgt_lang {
                                Language::De => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .title_de = Some(translated.clone());
                                    pipe_product.update.record.get_or_insert_default().title_de =
                                        Some(translated);
                                }
                                Language::En => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .title_en = Some(translated.clone());
                                    pipe_product.update.record.get_or_insert_default().title_en =
                                        Some(translated);
                                }
                                Language::Fr => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .title_fr = Some(translated.clone());
                                    pipe_product.update.record.get_or_insert_default().title_fr =
                                        Some(translated);
                                }
                                Language::Es => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .title_es = Some(translated.clone());
                                    pipe_product.update.record.get_or_insert_default().title_es =
                                        Some(translated);
                                }
                            }
                        } else {
                            error!(productId = %product_id, "Expected to find PipeProduct but didn't.");
                        }
                    }
                }
                Err(err) => {
                    error!(error = %err, srcLang = src_lang.as_str(), tgtLang = tgt_lang.as_str(), "Failed translating titles.");
                    failures.extend(product_ids_chunk.into_iter());
                    break;
                }
            }
        }
        failures
    }

    fn handle_translation_chunk_descriptions(
        &self,
        chunk: Chunk<'_, IntoIter<(ProductId, Description)>>,
        src_lang: &Language,
        products: &mut HashMap<ProductId, PipeProduct>,
    ) -> HashSet<ProductId> {
        let mut failures = HashSet::new();

        // these need to be Vec because we zip later!
        let (product_ids_chunk, native_descriptions_chunk): (Vec<_>, Vec<_>) =
            chunk.into_iter().unzip();
        let description_batch: Batch<String, 64> = Batch::try_from_iter(
            native_descriptions_chunk
                .into_iter()
                .map(|description| description.to_string()),
        )
        .expect(
            "shouldn't fail creating Batch of size 64 because 'itertools::chunks(16)' and 'Batch'
                share the same semantics and 16 < 64",
        );
        let tgt_langs = Language::iter().filter(|tgt| tgt != src_lang);
        for tgt_lang in tgt_langs {
            match self
                .translation_delegate
                .translate_batch(&description_batch, src_lang, &tgt_lang)
            {
                Ok(translated) => {
                    let translateds = product_ids_chunk
                        .iter()
                        .zip(translated.into_iter())
                        .collect::<HashMap<_, _>>();
                    for (product_id, translated) in translateds {
                        if let Some(pipe_product) = products.get_mut(product_id) {
                            match tgt_lang {
                                Language::De => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .description_de = Some(translated.clone());
                                    pipe_product
                                        .update
                                        .record
                                        .get_or_insert_default()
                                        .description_de = Some(translated);
                                }
                                Language::En => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .description_en = Some(translated.clone());
                                    pipe_product
                                        .update
                                        .record
                                        .get_or_insert_default()
                                        .description_en = Some(translated);
                                }
                                Language::Fr => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .description_fr = Some(translated.clone());
                                    pipe_product
                                        .update
                                        .record
                                        .get_or_insert_default()
                                        .description_fr = Some(translated);
                                }
                                Language::Es => {
                                    pipe_product
                                        .update
                                        .document
                                        .get_or_insert_default()
                                        .description_es = Some(translated.clone());
                                    pipe_product
                                        .update
                                        .record
                                        .get_or_insert_default()
                                        .description_es = Some(translated);
                                }
                            }
                        } else {
                            error!(productId = %product_id, "Expected to find PipeProduct but didn't.");
                        }
                    }
                }
                Err(err) => {
                    error!(error = %err, srcLang = src_lang.as_str(), tgtLang = tgt_lang.as_str(), "Failed translating descriptions.");
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
    use crate::{
        pipeline::{
            pipe::{EnrichmentPipe, PipeProduct, PipeProductSource, PipeProductUpdate},
            translate::{DESCRIPTION_BATCH_SIZE, TITLE_BATCH_SIZE, TranslationEnrichmentPipeImpl},
        },
        translate::MockTranslationDelegate,
    };
    use common::{language::domain::Language, localized::Localized};
    use fake::{Fake, Faker};
    use product::core::product_event::ProductCreatedEventPayload;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use std::sync::Arc;

    #[rstest::rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(42)]
    #[case(500)]
    #[case(1000)]
    fn should_never_use_single_translate(#[case] count: usize) {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate.expect_translate().never();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(fake::vec![String; 64].try_into().unwrap()));

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));
        let actual = enrichment_pipe.enrich(fake::vec![PipeProduct; count]);

        assert!(actual.failures.is_empty());
        assert_eq!(count, actual.successes.len());
    }

    #[test]
    fn should_enrich_titles_for_other_langs() {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Ok(vec!["Foo".to_string()].try_into().unwrap()));

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));

        let products = vec![PipeProduct {
            source: PipeProductSource {
                product_id: Faker.fake(),
                payload: ProductCreatedEventPayload {
                    shop_id: Faker.fake(),
                    shops_product_id: Faker.fake(),
                    shop_name: Faker.fake(),
                    native_title: Localized {
                        localization: Language::De,
                        payload: "Meow".into(),
                    },
                    native_description: Faker.fake(),
                    native_price: Faker.fake(),
                    other_price: Faker.fake(),
                    state: Faker.fake(),
                    url: Faker.fake(),
                    images: Faker.fake(),
                },
            },
            update: PipeProductUpdate::default(),
        }];
        let actuals = enrichment_pipe.enrich(products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual_1 = actuals.successes[0].clone().update;
        assert_eq!("Foo", actual_1.record.clone().unwrap().title_en.unwrap());
        assert_eq!("Foo", actual_1.document.clone().unwrap().title_en.unwrap());
    }

    #[test]
    fn should_not_enrich_title_for_id_lang() {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| {
                Ok(vec!["Foo".to_string(), "Bar".to_string()]
                    .try_into()
                    .unwrap())
            });

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));

        let products = vec![
            PipeProduct {
                source: PipeProductSource {
                    product_id: Faker.fake(),
                    payload: ProductCreatedEventPayload {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        shop_name: Faker.fake(),
                        native_title: Localized {
                            localization: Language::De,
                            payload: "Meow".into(),
                        },
                        native_description: Faker.fake(),
                        native_price: Faker.fake(),
                        other_price: Faker.fake(),
                        state: Faker.fake(),
                        url: Faker.fake(),
                        images: Faker.fake(),
                    },
                },
                update: PipeProductUpdate::default(),
            },
            PipeProduct {
                source: PipeProductSource {
                    product_id: Faker.fake(),
                    payload: ProductCreatedEventPayload {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        shop_name: Faker.fake(),
                        native_title: Localized {
                            localization: Language::De,
                            payload: "Wuff".into(),
                        },
                        native_description: Faker.fake(),
                        native_price: Faker.fake(),
                        other_price: Faker.fake(),
                        state: Faker.fake(),
                        url: Faker.fake(),
                        images: Faker.fake(),
                    },
                },
                update: PipeProductUpdate::default(),
            },
        ];
        let actuals = enrichment_pipe.enrich(products);

        assert!(actuals.failures.is_empty());
        assert_eq!(2, actuals.successes.len());

        let actual_1 = actuals.successes[0].clone().update;
        assert!(actual_1.record.clone().unwrap().title_de.is_none());
        assert!(actual_1.document.clone().unwrap().title_de.is_none());

        let actual_2 = actuals.successes[1].clone().update;
        assert!(actual_2.record.clone().unwrap().title_de.is_none());
        assert!(actual_2.document.clone().unwrap().title_de.is_none());
    }

    #[test]
    fn should_enrich_description_for_other_langs() {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| {
                Ok(vec!["Foo".to_string(), "Bar".to_string()]
                    .try_into()
                    .unwrap())
            });

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));

        let products = vec![PipeProduct {
            source: PipeProductSource {
                product_id: Faker.fake(),
                payload: ProductCreatedEventPayload {
                    shop_id: Faker.fake(),
                    shops_product_id: Faker.fake(),
                    shop_name: Faker.fake(),
                    native_title: Localized {
                        localization: Language::Fr,
                        payload: "Meh".into(),
                    },
                    native_description: Some(Localized {
                        localization: Language::En,
                        payload: "Meow".into(),
                    }),
                    native_price: Faker.fake(),
                    other_price: Faker.fake(),
                    state: Faker.fake(),
                    url: Faker.fake(),
                    images: Faker.fake(),
                },
            },
            update: PipeProductUpdate::default(),
        }];
        let actuals = enrichment_pipe.enrich(products);

        assert!(actuals.failures.is_empty());
        assert_eq!(1, actuals.successes.len());

        let actual_1 = actuals.successes[0].clone().update;
        assert_eq!(
            "Foo",
            actual_1.record.clone().unwrap().description_de.unwrap()
        );
        assert_eq!(
            "Foo",
            actual_1.document.clone().unwrap().description_de.unwrap()
        );
    }

    #[test]
    fn should_not_enrich_description_for_id_lang() {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| {
                Ok(vec!["Foo".to_string(), "Bar".to_string()]
                    .try_into()
                    .unwrap())
            });

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));

        let products = vec![
            PipeProduct {
                source: PipeProductSource {
                    product_id: Faker.fake(),
                    payload: ProductCreatedEventPayload {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        shop_name: Faker.fake(),
                        native_title: Localized {
                            localization: Language::Fr,
                            payload: "Meh".into(),
                        },
                        native_description: Some(Localized {
                            localization: Language::De,
                            payload: "Meow".into(),
                        }),
                        native_price: Faker.fake(),
                        other_price: Faker.fake(),
                        state: Faker.fake(),
                        url: Faker.fake(),
                        images: Faker.fake(),
                    },
                },
                update: PipeProductUpdate::default(),
            },
            PipeProduct {
                source: PipeProductSource {
                    product_id: Faker.fake(),
                    payload: ProductCreatedEventPayload {
                        shop_id: Faker.fake(),
                        shops_product_id: Faker.fake(),
                        shop_name: Faker.fake(),
                        native_title: Localized {
                            localization: Language::De,
                            payload: "Moh".into(),
                        },
                        native_description: Some(Localized {
                            localization: Language::De,
                            payload: "Wuff".into(),
                        }),
                        native_price: Faker.fake(),
                        other_price: Faker.fake(),
                        state: Faker.fake(),
                        url: Faker.fake(),
                        images: Faker.fake(),
                    },
                },
                update: PipeProductUpdate::default(),
            },
        ];
        let actuals = enrichment_pipe.enrich(products);

        assert!(actuals.failures.is_empty());
        assert_eq!(2, actuals.successes.len());

        let actual_1 = actuals.successes[0].clone().update;
        let actual_2 = actuals.successes[1].clone().update;

        assert!(actual_1.record.clone().unwrap().description_de.is_none());
        assert!(actual_1.document.clone().unwrap().description_de.is_none());

        assert!(actual_2.record.clone().unwrap().description_de.is_none());
        assert!(actual_2.document.clone().unwrap().description_de.is_none());
    }

    #[rstest::rstest]
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
    #[case(1000)]
    fn should_partially_fail(#[case] count: usize) {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|batch, _, _| {
                if batch.len() == TITLE_BATCH_SIZE || batch.len() == DESCRIPTION_BATCH_SIZE {
                    Ok(fake::vec![String; 64].try_into().unwrap())
                } else {
                    Err(PyErr::new::<PyTypeError, _>("Something went wrong"))
                }
            });

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));
        let mut products = fake::vec![PipeProduct; count];

        // we need to force the values of each title/description to have the same language
        // due to the grouping into a HashMap<Language, _> which varies bath_len
        // together with the expectation this now only ever fails the very last non-full batch
        for product in &mut products {
            product.source.payload.native_title = Localized {
                localization: Language::Es,
                payload: Faker.fake(),
            };
            product.source.payload.native_description = Some(Localized {
                localization: Language::De,
                payload: Faker.fake(),
            });
        }

        let actual = enrichment_pipe.enrich(products);

        assert_eq!(count - (count % 64), actual.successes.len());
        assert_eq!(count % 64, actual.failures.len());
    }

    #[rstest::rstest]
    #[case(0)]
    #[case(1)]
    #[case(2)]
    #[case(5)]
    #[case(10)]
    #[case(42)]
    #[case(500)]
    #[case(1000)]
    fn should_partially_fail_all(#[case] count: usize) {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));
        let actual = enrichment_pipe.enrich(fake::vec![PipeProduct; count]);

        assert!(actual.successes.is_empty());
        assert_eq!(count, actual.failures.len());
    }
}
