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
            let chunks = product_ids_native_titles.into_iter().chunks(64);
            for titles_chunk in chunks.into_iter() {
                let chunk_failures =
                    self.handle_translation_chunk_titles(titles_chunk, &lang, &mut products);
                failures.extend(chunk_failures);
            }
        }

        for (lang, product_ids_native_descriptions) in all_descriptions {
            let chunks = product_ids_native_descriptions.into_iter().chunks(32);
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

        let (product_ids_chunk, native_titles_chunk): (HashSet<_>, Vec<_>) =
            chunk.into_iter().unzip();
        let title_batch: Batch<String, 64> = Batch::try_from_iter(
            native_titles_chunk
                .into_iter()
                .map(|title| title.to_string()),
        )
        .expect(
            "shouldn't fail creating Batch of size 64 because 'itertools::chunks' and 'Batch'
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
                                Language::Es => {}
                                Language::Fr => {}
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

        let (product_ids_chunk, native_descriptions_chunk): (HashSet<_>, Vec<_>) =
            chunk.into_iter().unzip();
        let description_batch: Batch<String, 64> = Batch::try_from_iter(
            native_descriptions_chunk
                .into_iter()
                .map(|description| description.to_string()),
        )
        .expect(
            "shouldn't fail creating Batch of size 64 because 'itertools::chunks' and 'Batch'
                share the same semantics being invoked with same size",
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
                                Language::Es => {}
                                Language::Fr => {}
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
            pipe::{EnrichmentPipe, PipeProduct},
            translate::TranslationEnrichmentPipeImpl,
        },
        translate::MockTranslationDelegate,
    };
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
        let _ = enrichment_pipe.enrich(fake::vec![PipeProduct; count]);
    }

    #[test]
    fn should_enrich_title() {
        let mut translation_delegate = MockTranslationDelegate::default();
        translation_delegate
            .expect_translate_batch()
            .returning(|_, _, _| {
                Ok(vec!["foo".to_string(), "bar".to_string()]
                    .try_into()
                    .unwrap())
            });

        let enrichment_pipe = TranslationEnrichmentPipeImpl::new(Arc::new(translation_delegate));
    }
}
