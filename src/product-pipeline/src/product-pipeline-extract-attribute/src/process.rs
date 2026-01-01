use crate::{adapter::ExtractionAdapter, types::ExtractedAttributes};
use common::{batch::Batch, language::domain::Language};
use product_pipeline_common::{
    process::{PipeProcessor, ProcessResult},
    types::{AttributeExtractedPipeProduct, TextEmbeddedPipeProduct},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tracing::{error, info};

pub struct AttributeExtractionPipeProcesserImpl {
    extraction_delegate: Arc<dyn ExtractionAdapter + Send + Sync>,
}

impl AttributeExtractionPipeProcesserImpl {
    pub fn new(extraction_delegate: Arc<dyn ExtractionAdapter + Send + Sync>) -> Self {
        Self {
            extraction_delegate,
        }
    }
}

impl PipeProcessor<TextEmbeddedPipeProduct, AttributeExtractedPipeProduct>
    for AttributeExtractionPipeProcesserImpl
{
    fn process(
        &self,
        ins: Vec<TextEmbeddedPipeProduct>,
    ) -> ProcessResult<AttributeExtractedPipeProduct> {
        let count = ins.len();
        let mut successes = Vec::with_capacity(ins.len());
        let mut failures = HashSet::new();
        let batches: Vec<Batch<TextEmbeddedPipeProduct, 8>> = Batch::chunked_from(ins.into_iter());

        for in_batch in batches {
            let input_batch_iter = in_batch.iter().map(|in_product| {
                let mut titles: HashMap<Language, String> = in_product
                    .other_title
                    .iter()
                    .map(|(lang, s)| (Language::from(*lang), s.clone()))
                    .collect();
                titles.insert(
                    in_product.native_title.language.into(),
                    in_product.native_title.text.clone(),
                );
                let mut descriptions: HashMap<Language, String> = in_product
                    .other_description
                    .iter()
                    .map(|(lang, s)| (Language::from(*lang), s.clone()))
                    .collect();
                if let Some(ref native_description) = in_product.native_description {
                    descriptions.insert(
                        native_description.language.into(),
                        native_description.text.clone(),
                    );
                }
                format!(
                    "{}: {}",
                    Language::resolve(&[Language::En, Language::De], titles)
                        .as_ref()
                        .map(|localized| localized.payload.as_str())
                        .unwrap_or(""),
                    Language::resolve(&[Language::En, Language::De], descriptions)
                        .as_ref()
                        .map(|localized| localized.payload.as_str())
                        .unwrap_or(""),
                )
            });
            let input_batch = Batch::try_from_iter(input_batch_iter)
                .expect("shouldn't fail re-collecting former batch of same size");
            let schema = r#"
                {\n
                    "originYearMin": int | null (Lower end of the year-range, the antique is estimated to have originated from),\n
                    "originYearMax": int | null (Higher end of the year-range, the antique is estimated to have originated from),\n
                    "originYear": int | null (Exact year the antique is estimated to have originated from),\n
                    "authenticity": enum-string | null (The authenticity of the antique. Either of: ORIGINAL, LATER_COPY (antique copy), REPRODUCTION (modern copy), QUESTIONABLE, UNKNOWN),\n
                    "condition": enum-string | null (The condition of the antique. Either of: EXCELLENT, GREAT, GOOD, FAIR, POOR, UNKNOWN),\n
                    "provenance": enum-string | null (The documentation (trail) of the antique. Either of: COMPLETE, PARTIAL, CLAIMED (assumed, but no proof), NONE, UNKNOWN),\n
                    "restoration": enum-string | null (Restoration done to the antique. Either of: MAJOR, MINOR, NONE, UNKNOWN)\n
                }
            "#;

            match self.extraction_delegate.extract(schema, &input_batch) {
                Err(err) => {
                    error!(error = %err, "Failed delegating attribute-extraction.");
                    let mut local_failed = in_batch.iter().map(|in_product| in_product.product_id);
                    failures.extend(&mut local_failed);
                }
                Ok(extractions) => {
                    let mut local_enriched = in_batch.into_iter().zip(extractions.into_iter()).map(
                        |(in_product, extraction_str)| {
                            let mut extracted_attributes = serde_json::from_str(&extraction_str).unwrap_or_else(|err| {
                                error!(error = %err, adapterResponse = extraction_str, "Failed extracting attributes.");
                                ExtractedAttributes::default()
                            });
                            if let Some(origin_year_min) = extracted_attributes.origin_year_min && extracted_attributes.origin_year_min == extracted_attributes.origin_year_max {
                                extracted_attributes.origin_year = Some(origin_year_min);
                            }
                            if let Some(origin_year) = extracted_attributes.origin_year {
                                extracted_attributes.origin_year_min = Some(origin_year);
                                extracted_attributes.origin_year_max = Some(origin_year);
                            }
                            AttributeExtractedPipeProduct {
                                product_id: in_product.product_id,
                                shop_id: in_product.shop_id,
                                shops_product_id: in_product.shops_product_id,
                                native_title: in_product.native_title,
                                other_title: in_product.other_title,
                                native_description: in_product.native_description,
                                other_description: in_product.other_description,
                                text_embedding: in_product.text_embedding,
                                origin_year_min: extracted_attributes.origin_year_min,
                                origin_year: extracted_attributes.origin_year,
                                origin_year_max: extracted_attributes.origin_year_max,
                                authenticity: extracted_attributes.authenticity.unwrap_or_default(),
                                condition: extracted_attributes.condition.unwrap_or_default(),
                                provenance: extracted_attributes.provenance.unwrap_or_default(),
                                restoration: extracted_attributes.restoration.unwrap_or_default(),
                            }
                        },
                    );
                    successes.extend(&mut local_enriched);
                }
            }
        }

        info!(
            count = count,
            successes = count - failures.len(),
            failures = failures.len(),
            "Extracted attributes."
        );

        ProcessResult {
            successes,
            failures,
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{adapter::MockExtractionAdapter, process::AttributeExtractionPipeProcesserImpl};
    use product::dynamodb::condition_record::ConditionRecord;
    use product_pipeline_common::{process::PipeProcessor, types::TextEmbeddedPipeProduct};
    use pyo3::{PyErr, exceptions::PyTypeError};
    use std::sync::Arc;

    #[test]
    fn should_keep_order_of_delegate_returned_extractions() {
        let mock_res = vec![
            r#"{"condition": "EXCELLENT"}"#.to_owned(),
            r#"{"condition": "POOR"}"#.to_owned(),
            r#"{"condition": "UNKNOWN"}"#.to_owned(),
            r#"{"condition": "FAIR"}"#.to_owned(),
            r#"{"condition": "GREAT"}"#.to_owned(),
        ];
        let mut delegate = MockExtractionAdapter::default();
        delegate
            .expect_extract()
            .return_once(move |_, _| Ok(mock_res.try_into().unwrap()));

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe.process(fake::vec![TextEmbeddedPipeProduct; 5]);
        let actual = res
            .successes
            .into_iter()
            .map(|out_product| out_product.condition)
            .collect::<Vec<_>>();

        let expected = vec![
            ConditionRecord::Excellent,
            ConditionRecord::Poor,
            ConditionRecord::Unknown,
            ConditionRecord::Fair,
            ConditionRecord::Great,
        ];
        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

    #[rstest::rstest]
    #[trace]
    #[case(1)]
    #[case(5)]
    #[case(20)]
    #[case(32)]
    #[case(64)]
    #[case(70)]
    #[case(128)]
    #[case(129)]
    #[case(256)]
    #[case(1000)]
    #[case(1500)]
    fn should_partially_fail_entire_batches(#[case] input_count: usize) {
        let mut delegate = MockExtractionAdapter::default();
        delegate
            .expect_extract()
            .returning(move |_, _| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe.process(fake::vec![TextEmbeddedPipeProduct; input_count]);

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
    }
}
