//! Product title translation through Claude: groups of titles in provider batches, and single
//! titles on the refusal fallback model.

mod prompt;
mod response;

use application::error::box_error;
use indexmap::IndexMap;
use large_language_model::{
    BatchCustomId, BatchItemOutcome, BatchProcessingStatus, BatchRequest, LargeLanguageModel,
    LargeLanguageModelBatchId, LargeLanguageModelBatches, LargeLanguageModelError, LlmOperation,
    StructuredResponseFailureKind, log_llm_refusal_fallback,
};
use localization::Language;
use product_listing_core::title::Title;
use product_listing_service::ports::{
    PositionedTitle, ProductListingTitleBatchTranslator, ProductListingTitleTranslationError,
    ProductListingTitleTranslator, TitleTranslationBatchId, TitleTranslationBatchPoll,
    TitleTranslationGroup, TitleTranslationGroupOutcome, TitleTranslationGroupResult,
};
use response::GroupResponse;

const OPERATION: LlmOperation = LlmOperation::ProductTitleTranslation;

/// Translates title groups through a provider batch on the primary model.
pub struct LargeLanguageModelProductListingTitleBatchTranslator<M> {
    model: M,
}

impl<M> LargeLanguageModelProductListingTitleBatchTranslator<M> {
    pub fn new(model: M) -> Self {
        Self { model }
    }
}

#[async_trait::async_trait]
impl<M> ProductListingTitleBatchTranslator
    for LargeLanguageModelProductListingTitleBatchTranslator<M>
where
    M: LargeLanguageModelBatches,
{
    fn group_sizes(
        &self,
        _source_language: Language,
        target_languages: &[Language],
        titles: &[Title],
    ) -> Vec<usize> {
        prompt::group_sizes(target_languages, titles)
    }

    async fn submit(
        &self,
        groups: &[TitleTranslationGroup],
    ) -> Result<TitleTranslationBatchId, ProductListingTitleTranslationError> {
        let requests = groups
            .iter()
            .map(|group| {
                Ok(BatchRequest {
                    custom_id: custom_id(group.group)?,
                    request: prompt::translation_request(group),
                })
            })
            .collect::<Result<Vec<_>, LargeLanguageModelError>>()
            .map_err(map_translation_error)?;
        let batch = self
            .model
            .submit_batch(OPERATION, requests)
            .await
            .map_err(map_translation_error)?;
        TitleTranslationBatchId::new(batch.as_str()).map_err(|source| {
            ProductListingTitleTranslationError::InvalidResponse {
                source: box_error(source),
            }
        })
    }

    async fn poll(
        &self,
        batch: &TitleTranslationBatchId,
        groups: &[TitleTranslationGroup],
    ) -> Result<TitleTranslationBatchPoll, ProductListingTitleTranslationError> {
        let batch =
            LargeLanguageModelBatchId::new(batch.as_str()).map_err(map_translation_error)?;
        if self
            .model
            .batch_status(&batch)
            .await
            .map_err(map_translation_error)?
            == BatchProcessingStatus::InProgress
        {
            return Ok(TitleTranslationBatchPoll::InProgress);
        }
        let results = self
            .model
            .batch_results::<GroupResponse>(OPERATION, &batch)
            .await
            .map_err(map_translation_error)?;
        Ok(TitleTranslationBatchPoll::Ended(
            results
                .into_iter()
                .filter_map(|result| {
                    let group = groups
                        .iter()
                        .find(|group| custom_id_matches(group.group, &result.custom_id))?;
                    Some(TitleTranslationGroupResult {
                        group: group.group,
                        outcome: group_outcome(group, result.outcome),
                    })
                })
                .collect(),
        ))
    }
}

fn custom_id(group: u32) -> Result<BatchCustomId, LargeLanguageModelError> {
    BatchCustomId::new(format!("g{group}"))
}

fn custom_id_matches(group: u32, custom_id: &str) -> bool {
    custom_id
        .strip_prefix('g')
        .and_then(|number| number.parse::<u32>().ok())
        == Some(group)
}

fn group_outcome(
    group: &TitleTranslationGroup,
    outcome: BatchItemOutcome<GroupResponse>,
) -> TitleTranslationGroupOutcome {
    match outcome {
        BatchItemOutcome::Succeeded(response) => TitleTranslationGroupOutcome::Translated(
            group
                .titles
                .iter()
                .map(|PositionedTitle { position, title }| {
                    (
                        *position,
                        response.translated_title(*position, title, &group.target_languages),
                    )
                })
                .collect(),
        ),
        BatchItemOutcome::Failed(error) if error.is_refusal() => {
            TitleTranslationGroupOutcome::Refused
        }
        BatchItemOutcome::Failed(error)
            if error.structured_response_failure_kind()
                == Some(StructuredResponseFailureKind::MaxTokens) =>
        {
            TitleTranslationGroupOutcome::Truncated
        }
        BatchItemOutcome::Failed(error) => {
            TitleTranslationGroupOutcome::Failed(map_translation_error(error))
        }
        BatchItemOutcome::NotProcessed => TitleTranslationGroupOutcome::NotProcessed,
    }
}

/// Translates one title the primary model refused, synchronously, on the refusal fallback model.
/// Haiku's safety classifier refuses some old scientific titles (pathogens, aerosols) that Sonnet
/// translates (#3076).
pub struct LargeLanguageModelRefusalFallbackTitleTranslator<M> {
    model: M,
}

impl<M> LargeLanguageModelRefusalFallbackTitleTranslator<M> {
    pub fn new(model: M) -> Self {
        Self { model }
    }
}

#[async_trait::async_trait]
impl<M> ProductListingTitleTranslator for LargeLanguageModelRefusalFallbackTitleTranslator<M>
where
    M: LargeLanguageModel,
{
    async fn translate(
        &self,
        title: &Title,
        source_language: Language,
        target_languages: &[Language],
    ) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
        let group = TitleTranslationGroup {
            group: 1,
            source_language,
            target_languages: target_languages.to_vec(),
            titles: vec![PositionedTitle {
                position: 1,
                title: title.clone(),
            }],
        };
        let response = self
            .model
            .generate::<GroupResponse>(prompt::translation_request(&group))
            .await;
        // The primary's refusal was logged with its batch.
        log_llm_refusal_fallback(
            OPERATION,
            self.model.logged_model(),
            response.as_ref().err(),
        );
        response
            .map_err(map_translation_error)?
            .translated_title(1, title, target_languages)
    }
}

fn map_translation_error(error: LargeLanguageModelError) -> ProductListingTitleTranslationError {
    if error.is_refusal() {
        return ProductListingTitleTranslationError::Refused {
            source: box_error(error),
        };
    }
    match error {
        LargeLanguageModelError::Timeout { .. }
        | LargeLanguageModelError::Retryable { .. }
        | LargeLanguageModelError::Authentication { .. } => {
            ProductListingTitleTranslationError::TemporarilyUnavailable {
                source: box_error(error),
            }
        }
        LargeLanguageModelError::InvalidRequest { .. }
        | LargeLanguageModelError::Permanent { .. }
        | LargeLanguageModelError::InvalidResponse { .. } => {
            ProductListingTitleTranslationError::InvalidResponse {
                source: box_error(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use large_language_model::{
        BatchItemResult, LargeLanguageModelRetryAdvice, LargeLanguageModelRetryKind,
        StructuredGenerationRequest,
    };
    use serde::de::DeserializeOwned;
    use std::sync::Mutex;

    fn group(group: u32, titles: &[&str]) -> TitleTranslationGroup {
        TitleTranslationGroup {
            group,
            source_language: Language::De,
            target_languages: vec![Language::En],
            titles: (1..)
                .zip(titles)
                .map(|(position, title)| PositionedTitle {
                    position,
                    title: Title::from(*title),
                })
                .collect(),
        }
    }

    /// A batch that has ended with scripted JSON results, recording what was submitted.
    #[derive(Default)]
    struct ScriptedBatches {
        results: Mutex<Vec<(String, Result<serde_json::Value, LargeLanguageModelError>)>>,
        submitted: Mutex<Vec<BatchRequest>>,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModelBatches for ScriptedBatches {
        async fn submit_batch(
            &self,
            _: LlmOperation,
            requests: Vec<BatchRequest>,
        ) -> Result<LargeLanguageModelBatchId, LargeLanguageModelError> {
            *self.submitted.lock().unwrap() = requests;
            LargeLanguageModelBatchId::new("msgbatch_1")
        }

        async fn batch_status(
            &self,
            _: &LargeLanguageModelBatchId,
        ) -> Result<BatchProcessingStatus, LargeLanguageModelError> {
            Ok(BatchProcessingStatus::Ended)
        }

        async fn batch_results<Output>(
            &self,
            _: LlmOperation,
            _: &LargeLanguageModelBatchId,
        ) -> Result<Vec<BatchItemResult<Output>>, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            Ok(std::mem::take(&mut *self.results.lock().unwrap())
                .into_iter()
                .map(|(custom_id, result)| BatchItemResult {
                    custom_id,
                    outcome: match result {
                        Ok(value) => {
                            BatchItemOutcome::Succeeded(serde_json::from_value(value).unwrap())
                        }
                        Err(error) => BatchItemOutcome::Failed(error),
                    },
                })
                .collect())
        }
    }

    async fn poll(
        batches: ScriptedBatches,
        groups: &[TitleTranslationGroup],
    ) -> Vec<TitleTranslationGroupResult> {
        let translator = LargeLanguageModelProductListingTitleBatchTranslator::new(batches);
        match translator
            .poll(&TitleTranslationBatchId::new("msgbatch_1").unwrap(), groups)
            .await
            .unwrap()
        {
            TitleTranslationBatchPoll::Ended(results) => results,
            TitleTranslationBatchPoll::InProgress => panic!("batch ended"),
        }
    }

    #[tokio::test]
    async fn should_submit_one_request_per_group_keyed_by_group() {
        let translator =
            LargeLanguageModelProductListingTitleBatchTranslator::new(ScriptedBatches::default());

        let id = translator
            .submit(&[group(1, &["Stuhl", "Tisch"]), group(2, &["Vase"])])
            .await
            .unwrap();

        assert_eq!("msgbatch_1", id.as_str());
        let submitted = translator.model.submitted.lock().unwrap();
        assert_eq!(
            vec!["g1", "g2"],
            submitted
                .iter()
                .map(|request| request.custom_id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(submitted[0].request.prompt.contains(r#""2":"Tisch""#));
    }

    #[tokio::test]
    async fn should_validate_every_title_of_a_group_on_its_own() {
        let batches = ScriptedBatches::default();
        *batches.results.lock().unwrap() = vec![(
            "g7".to_owned(),
            Ok(serde_json::json!({"translations": {
                "1": {"en": "Oak chair"},
                "3": {"en": ""},
            }})),
        )];

        let results = poll(batches, &[group(7, &["Eichenstuhl", "Tisch", "Vase"])]).await;

        let [
            TitleTranslationGroupResult {
                group: 7,
                outcome: TitleTranslationGroupOutcome::Translated(titles),
            },
        ] = results.as_slice()
        else {
            panic!("one translated group expected: {results:?}");
        };
        assert_eq!(3, titles.len());
        assert_eq!(
            Some("Oak chair"),
            titles[0]
                .1
                .as_ref()
                .ok()
                .and_then(|titles| titles.get(&Language::En))
                .map(AsRef::as_ref)
        );
        assert!(titles[1].1.is_err(), "an omitted title fails alone");
        assert!(titles[2].1.is_err(), "an empty title fails alone");
    }

    #[tokio::test]
    async fn should_tell_refused_groups_from_failed_ones() {
        let batches = ScriptedBatches::default();
        *batches.results.lock().unwrap() = vec![
            ("g1".to_owned(), Err(LargeLanguageModelError::refusal())),
            (
                "g2".to_owned(),
                Err(LargeLanguageModelError::Retryable {
                    advice: LargeLanguageModelRetryAdvice {
                        kind: LargeLanguageModelRetryKind::Transient,
                        retry_after: None,
                    },
                    source: static_error("api error"),
                }),
            ),
            (
                "unknown".to_owned(),
                Err(LargeLanguageModelError::refusal()),
            ),
        ];

        let results = poll(batches, &[group(1, &["Pest"]), group(2, &["Tisch"])]).await;

        assert_eq!(2, results.len(), "an unknown custom ID is ignored");
        assert!(matches!(
            results[0].outcome,
            TitleTranslationGroupOutcome::Refused
        ));
        assert!(matches!(
            results[1].outcome,
            TitleTranslationGroupOutcome::Failed(
                ProductListingTitleTranslationError::TemporarilyUnavailable { .. }
            )
        ));
    }

    /// Answers each call with the next scripted result and records the requests it received.
    #[derive(Default)]
    struct ScriptedModel {
        results: Mutex<Vec<Result<serde_json::Value, LargeLanguageModelError>>>,
        requests: Mutex<Vec<StructuredGenerationRequest>>,
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for ScriptedModel {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            self.requests.lock().unwrap().push(request);
            self.results
                .lock()
                .unwrap()
                .pop()
                .expect("unexpected model call")
                .map(|value| serde_json::from_value(value).unwrap())
        }
    }

    #[tokio::test]
    async fn should_translate_a_refused_title_alone_on_the_fallback_and_report_its_refusal() {
        let model = ScriptedModel::default();
        model.results.lock().unwrap().push(Ok(serde_json::json!({
            "translations": {"1": {"en": "Yersinia pestis protocols"}}
        })));
        let fallback = LargeLanguageModelRefusalFallbackTitleTranslator::new(model);

        let translated = fallback
            .translate(
                &Title::from("Yersinia Pestis Protokolle"),
                Language::De,
                &[Language::En],
            )
            .await
            .unwrap();

        assert_eq!(
            Some("Yersinia pestis protocols"),
            translated.get(&Language::En).map(AsRef::as_ref)
        );
        let prompt = fallback.model.requests.lock().unwrap()[0].prompt.clone();
        assert!(prompt.contains(r#"{"1":"Yersinia Pestis Protokolle"}"#));

        let model = ScriptedModel::default();
        model
            .results
            .lock()
            .unwrap()
            .push(Err(LargeLanguageModelError::refusal()));
        assert!(matches!(
            LargeLanguageModelRefusalFallbackTitleTranslator::new(model)
                .translate(&Title::from("Pest"), Language::De, &[Language::En])
                .await,
            Err(ProductListingTitleTranslationError::Refused { .. })
        ));
    }
}
