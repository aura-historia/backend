use crate::{adapter::ExtractionAdapter, types::ExtractedAttributes};
use common::{batch::Batch, year::YearRange};
use product::core::{
    origin_year::OriginYear,
    product::Product,
    product_event::ProductEventPayload,
    prohibited_content::{ProhibitedContent, ProhibitedContentReason},
};
use product_pipeline_common::process::{PipeProcessor, ProcessResult};
use std::{collections::HashSet, sync::Arc};
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

#[async_trait::async_trait]
impl PipeProcessor for AttributeExtractionPipeProcesserImpl {
    async fn process(&self, ins: Vec<Product>) -> ProcessResult {
        let count = ins.len();
        let mut successes = Vec::with_capacity(2 * count);
        let mut failures = HashSet::new();
        // Sort by text length so each batch contains items of similar length,
        // reducing padding overhead in transformer models.
        let mut sorted_ins = ins;
        sorted_ins.sort_by_key(|product| {
            product.native_title.payload.len()
                + product
                    .native_description
                    .as_ref()
                    .map_or(0, |d| d.payload.len())
        });
        let batches: Vec<Batch<Product, 8>> = Batch::chunked_from(sorted_ins.into_iter());

        for in_batch in batches {
            let input_batch_iter = in_batch.iter().map(|product| {
                format!(
                    "{}: {}",
                    product.native_title.payload,
                    product
                        .native_description
                        .clone()
                        .map(|description| description.payload)
                        .unwrap_or("".into()),
                )
            });
            let input_batch = Batch::try_from_iter(input_batch_iter)
                .expect("shouldn't fail re-collecting former batch of same size");
            let schema = r#"
                A note on centuries below.
                If noted early, e.g. early 18th century, then extract min=1800 and max=1833.
                If noted mid, e.g. mid 16th century, then extract min=1634 and max=1666.
                If noted late, e.g. late 17th century, then extract min=1767 and max=1799.
                If an exact year can be extracted, leave min and max null.
                {\n
                    "originYearMin": int | null (Lower end of the year-range, the antique is from),\n
                    "originYearMax": int | null (Higher end of the year-range, the antique is from),\n
                    "originYear": int | null (Exact year the antique is from),\n
                    "authenticity": enum-string | null (The authenticity of the antique. Either of: ORIGINAL, LATER_COPY (antique copy), REPRODUCTION (modern copy), QUESTIONABLE, UNKNOWN),\n
                    "condition": enum-string | null (The condition of the antique. Either of: EXCELLENT, GREAT, GOOD, FAIR, POOR, UNKNOWN),\n
                    "provenance": enum-string | null (The documentation (trail) of the antique. Either of: COMPLETE, PARTIAL, CLAIMED (assumed, but no proof), NONE, UNKNOWN),\n
                    "restoration": enum-string | null (Restoration done to the antique. Either of: MAJOR, MINOR, NONE, UNKNOWN),\n
                    "isFromNaziGermanyEpoch": bool (Whether the antique is from or related to the German Nazis in the 20th Century. Things like SA/SS pre 1933 also count into this.)\n
                }
            "#;

            match self.extraction_delegate.extract(schema, &input_batch) {
                Err(err) => {
                    error!(error = %err, "Failed delegating attribute-extraction.");
                    let mut local_failed = in_batch.iter().map(|in_product| in_product.product_id);
                    failures.extend(&mut local_failed);
                }
                Ok(extractions) => {
                    let zipped = in_batch.into_iter().zip(extractions.into_iter());
                    for (mut product, extraction_str) in zipped {
                        let cleaned_extraction_str = extraction_str
                            .chars()
                            .skip_while(|c| c != &'{')
                            .collect::<String>();
                        match serde_json::from_str::<ExtractedAttributes>(&cleaned_extraction_str) {
                            Ok(extracted_attributes) => {
                                let origin_year = match (
                                    extracted_attributes.origin_year_min,
                                    extracted_attributes.origin_year,
                                    extracted_attributes.origin_year_max,
                                ) {
                                    (_, Some(exact), _) => Some(OriginYear::ExactYear(exact)),
                                    (Some(min), None, Some(max)) => {
                                        if min == max {
                                            Some(OriginYear::ExactYear(min))
                                        } else {
                                            Some(OriginYear::EstimatedRange(YearRange {
                                                min: Some(min),
                                                max: Some(max),
                                            }))
                                        }
                                    }
                                    (min @ Some(_), None, max) => {
                                        Some(OriginYear::EstimatedRange(YearRange { min, max }))
                                    }
                                    (min, None, max @ Some(_)) => {
                                        Some(OriginYear::EstimatedRange(YearRange { min, max }))
                                    }
                                    (None, None, None) => None,
                                };
                                if let Some(extract_event) = product.extract_attributes(
                                    origin_year,
                                    extracted_attributes.authenticity.map(Into::into),
                                    extracted_attributes.condition.map(Into::into),
                                    extracted_attributes.provenance.map(Into::into),
                                    extracted_attributes.restoration.map(Into::into),
                                ) {
                                    successes
                                        .push(extract_event.map_payload(ProductEventPayload::from));
                                }
                                let prohibited_content_event_opt =
                                    match extracted_attributes.is_from_nazi_germany_epoch {
                                        None => None,
                                        Some(false) => product.prohibit_content(
                                            ProhibitedContent::None,
                                            ProhibitedContentReason::ProductText,
                                        ),
                                        Some(true) => product.prohibit_content(
                                            ProhibitedContent::NaziGermany,
                                            ProhibitedContentReason::ProductText,
                                        ),
                                    };
                                if let Some(prohibited_content_event) = prohibited_content_event_opt
                                {
                                    successes.push(
                                        prohibited_content_event
                                            .map_payload(ProductEventPayload::from),
                                    );
                                }
                            }
                            Err(err) => {
                                error!(error = %err, adapterResponse = extraction_str, "Failed extracting attributes.");
                                failures.insert(product.product_id);
                            }
                        }
                    }
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
    use common::batch::Batch;
    use product::core::{condition::Condition, product::Product};
    use product_pipeline_common::process::PipeProcessor;
    use pyo3::{PyErr, exceptions::PyTypeError};
    use std::sync::Arc;

    #[tokio::test]
    async fn should_keep_order_of_delegate_returned_extractions() {
        let mock_res = vec![
            r#"{"condition": "EXCELLENT"}"#.to_owned(),
            r#"{"condition": "POOR"}"#.to_owned(),
            r#"{"condition": "GOOD"}"#.to_owned(),
            r#"{"condition": "FAIR"}"#.to_owned(),
            r#"{"condition": "GREAT"}"#.to_owned(),
        ];
        let mut delegate = MockExtractionAdapter::default();
        delegate
            .expect_extract()
            .return_once(move |_, _| Ok(mock_res.try_into().unwrap()));

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let mut products = fake::vec![Product; 5];
        for product in &mut products {
            product.condition = Default::default();
        }
        let res = embedding_pipe.process(products).await;
        let actual = res
            .successes
            .into_iter()
            .map(|event| {
                event
                    .payload
                    .as_enrichment_event()
                    .unwrap()
                    .as_extracted_attributes()
                    .unwrap()
                    .condition
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let expected = vec![
            Condition::Excellent,
            Condition::Poor,
            Condition::Good,
            Condition::Fair,
            Condition::Great,
        ];
        assert_eq!(expected, actual);
        assert!(res.failures.is_empty());
    }

    #[rstest::rstest]
    #[case("")]
    #[case("</think>")]
    #[case("</think>\n")]
    #[case("</think>\n\n")]
    #[case("<think></think>")]
    #[case("<think></think>\n")]
    #[case("<think></think>\n\n")]
    #[case("<think>foo</think>")]
    #[case("<think>foo</think>\n")]
    #[case("<think>foo</think>\n\n")]
    #[case("Whatever bro might be yapping here. I don't care.")]
    #[case("Whatever bro might be yapping here. I don't care.\n")]
    #[case("Whatever bro might be yapping here. I don't care.\n\n")]
    #[tokio::test]
    async fn should_extract_when_model_response_contains_anything_before_actual_json(
        #[case] prefix: String,
    ) {
        let mock_res = vec![format!("{prefix}{}", r#"{"condition": "EXCELLENT"}"#)];
        let mut delegate = MockExtractionAdapter::default();
        delegate
            .expect_extract()
            .return_once(move |_, _| Ok(mock_res.try_into().unwrap()));

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let mut products = fake::vec![Product; 1];
        for product in &mut products {
            product.condition = Default::default();
        }
        let res = embedding_pipe.process(products).await;
        let actual = res
            .successes
            .into_iter()
            .map(|event| {
                event
                    .payload
                    .as_enrichment_event()
                    .unwrap()
                    .as_extracted_attributes()
                    .unwrap()
                    .condition
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let expected = vec![Condition::Excellent];
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
    #[tokio::test]
    async fn should_partially_fail_entire_batches_when_py_err(#[case] input_count: usize) {
        let mut delegate = MockExtractionAdapter::default();
        delegate
            .expect_extract()
            .returning(move |_, _| Err(PyErr::new::<PyTypeError, _>("Something went wrong")));

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe
            .process(fake::vec![Product; input_count])
            .await;

        assert!(res.successes.is_empty());
        assert_eq!(input_count, res.failures.len());
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
    #[tokio::test]
    async fn should_partially_fail_batch_elements_when_parse_err(#[case] input_count: usize) {
        let mut delegate = MockExtractionAdapter::default();
        delegate.expect_extract().returning(move |_, batch| {
            let mock_res = vec![
                r#"{"invalid json"}"#.to_owned(),
                r#"{"condition": "POOR"}"#.to_owned(),
                r#"{"condition": "UNKNOWN"}"#.to_owned(),
                r#"{"condition": "FAIR"}"#.to_owned(),
                r#"{"condition": "GREAT"}"#.to_owned(),
                r#"{"condition": "GREAT"}"#.to_owned(),
                r#"{"condition": "GREAT"}"#.to_owned(),
                r#"{"condition": "GREAT"}"#.to_owned(),
            ]
            .into_iter()
            .take(batch.len());
            Ok(Batch::try_from_iter(mock_res).unwrap())
        });

        let embedding_pipe = AttributeExtractionPipeProcesserImpl::new(Arc::new(delegate));
        let res = embedding_pipe
            .process(fake::vec![Product; input_count])
            .await;

        let expected_failures = input_count.div_ceil(8);
        assert_eq!(expected_failures, res.failures.len());
        assert_eq!(input_count - expected_failures, res.successes.len());
    }
}
