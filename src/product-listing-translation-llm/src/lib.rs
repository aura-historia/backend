use application::error::{box_error, static_error};
use indexmap::IndexMap;
use large_language_model::{
    GenerationOptions, LargeLanguageModel, LargeLanguageModelError, LlmOperation,
    StructuredGenerationRequest, log_llm_refusal_fallback,
};
use localization::Language;
use product_listing_core::title::Title;
use product_listing_service::ports::{
    ProductListingTitleTranslationError, ProductListingTitleTranslator,
};
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};

/// Output budget for one title translated into every target. A dense Chinese or Japanese title
/// of up to [`Title::MAX_CHARS`] characters expands to several hundred characters in each of up
/// to five Latin-script targets, about 900 tokens with the JSON. 512 tokens truncated such
/// replies, which then failed on every attempt. Output is billed only for the tokens produced.
const TRANSLATION_MAX_OUTPUT_TOKENS: u16 = 2048;

const TRANSLATION_SYSTEM_INSTRUCTION: &str = "Translate antique product titles faithfully. Preserve proper nouns, periods, dimensions, and material names.";

/// Translates titles with `primary`. A refusal repeats for the same request and model, so only a
/// refusal is retried, once, on `refusal_fallback`. Every other failure is reported as the
/// primary returned it: the fallback is not a general retry path.
pub struct LargeLanguageModelProductListingTitleTranslator<P, F> {
    primary: P,
    refusal_fallback: F,
}

impl<P, F> LargeLanguageModelProductListingTitleTranslator<P, F> {
    pub fn new(primary: P, refusal_fallback: F) -> Self {
        Self {
            primary,
            refusal_fallback,
        }
    }
}

impl<P, F> LargeLanguageModelProductListingTitleTranslator<P, F>
where
    P: LargeLanguageModel,
    F: LargeLanguageModel,
{
    async fn generate(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<TranslationResponse, LargeLanguageModelError> {
        match self.primary.generate(request.clone()).await {
            // The fallback's own failure decides the job's retry; the primary's refusal is
            // already logged as its `LLM_INVOCATION` failure.
            Err(error) if error.is_refusal() => {
                let operation = request.operation;
                let result = self.refusal_fallback.generate(request).await;
                log_llm_refusal_fallback(
                    operation,
                    self.refusal_fallback.logged_model(),
                    result.as_ref().err(),
                );
                result
            }
            result => result,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TranslationResponse {
    titles: BTreeMap<String, String>,
}

#[async_trait::async_trait]
impl<P, F> ProductListingTitleTranslator for LargeLanguageModelProductListingTitleTranslator<P, F>
where
    P: LargeLanguageModel,
    F: LargeLanguageModel,
{
    async fn translate(
        &self,
        title: &Title,
        source_language: Language,
        target_languages: &[Language],
    ) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
        let response = self
            .generate(translation_request(
                title,
                source_language,
                target_languages,
            ))
            .await
            .map_err(map_translation_error)?;
        translated_titles(response, target_languages)
    }
}

/// Fails closed unless the response holds a non-empty title for every requested target.
fn translated_titles(
    response: TranslationResponse,
    target_languages: &[Language],
) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
    let mut translated = IndexMap::new();
    for language in target_languages {
        let Some(value) = response.titles.get(language.as_str()) else {
            return Err(ProductListingTitleTranslationError::InvalidResponse {
                source: static_error(
                    "product title translation response omitted a target language",
                ),
            });
        };
        let title = Title::from(value.as_str());
        if title.as_ref().is_empty() {
            return Err(ProductListingTitleTranslationError::InvalidResponse {
                source: static_error("product title translation response contains an empty title"),
            });
        }
        translated.insert(*language, title);
    }
    Ok(translated)
}

fn translation_request(
    title: &Title,
    source_language: Language,
    target_languages: &[Language],
) -> StructuredGenerationRequest {
    let targets = target_languages
        .iter()
        .map(|language| language.as_str())
        .collect::<Vec<_>>();
    StructuredGenerationRequest {
        operation: LlmOperation::ProductTitleTranslation,
        system_instruction: TRANSLATION_SYSTEM_INSTRUCTION.to_owned(),
        prompt: format!(
            "Translate this antique product title from {source} into {targets}. Return exactly one JSON object with a titles object whose keys are the target ISO language codes and whose values are non-empty translated titles.\n\nTitle: {title}",
            source = source_language.as_str(),
            targets = targets.join(", "),
            title = title.as_ref(),
        ),
        image_urls: Vec::new(),
        response_json_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "titles": {
                    "type": "object",
                    "properties": targets.iter().map(|language| (
                        (*language).to_owned(),
                        serde_json::json!({ "type": "string" }),
                    )).collect::<serde_json::Map<String, serde_json::Value>>(),
                    "required": targets,
                }
            },
            "required": ["titles"],
        }),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: TRANSLATION_MAX_OUTPUT_TOKENS,
            request_timeout: Duration::from_secs(30),
        },
    }
}

fn map_translation_error(error: LargeLanguageModelError) -> ProductListingTitleTranslationError {
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
    use large_language_model::{LargeLanguageModelRetryAdvice, LargeLanguageModelRetryKind};
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    /// Answers each call with the next scripted result and records the requests it received.
    #[derive(Clone, Default)]
    struct ScriptedModel {
        results: Arc<Mutex<VecDeque<Result<serde_json::Value, LargeLanguageModelError>>>>,
        requests: Arc<Mutex<Vec<StructuredGenerationRequest>>>,
    }

    impl ScriptedModel {
        fn answering(
            results: impl IntoIterator<Item = Result<serde_json::Value, LargeLanguageModelError>>,
        ) -> Self {
            Self {
                results: Arc::new(Mutex::new(results.into_iter().collect())),
                requests: Arc::default(),
            }
        }

        fn prompts(&self) -> Vec<String> {
            self.requests
                .lock()
                .expect("requests lock")
                .iter()
                .map(|request| request.prompt.clone())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl LargeLanguageModel for ScriptedModel {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            self.requests.lock().expect("requests lock").push(request);
            let result = self
                .results
                .lock()
                .expect("results lock")
                .pop_front()
                .expect("unexpected model call");
            result.map(|value| serde_json::from_value(value).expect("scripted output"))
        }
    }

    fn english_titles() -> serde_json::Value {
        serde_json::json!({"titles": {"en": "Yersinia pestis protocols"}})
    }

    async fn translate(
        primary: &ScriptedModel,
        refusal_fallback: &ScriptedModel,
    ) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
        LargeLanguageModelProductListingTitleTranslator::new(
            primary.clone(),
            refusal_fallback.clone(),
        )
        .translate(
            &Title::from("Yersinia Pestis Protokolle"),
            Language::De,
            &[Language::En],
        )
        .await
    }

    #[tokio::test]
    async fn should_retry_the_same_request_on_the_fallback_when_the_primary_refuses() {
        let primary = ScriptedModel::answering([Err(LargeLanguageModelError::refusal())]);
        let fallback = ScriptedModel::answering([Ok(english_titles())]);

        let translated = translate(&primary, &fallback).await.expect("translated");

        assert_eq!(
            Some("Yersinia pestis protocols"),
            translated.get(&Language::En).map(AsRef::as_ref)
        );
        assert_eq!(primary.prompts(), fallback.prompts());
    }

    #[tokio::test]
    async fn should_fail_without_another_attempt_when_the_fallback_refuses_too() {
        let primary = ScriptedModel::answering([Err(LargeLanguageModelError::refusal())]);
        let fallback = ScriptedModel::answering([Err(LargeLanguageModelError::refusal())]);

        let error = translate(&primary, &fallback).await.unwrap_err();

        assert!(matches!(
            error,
            ProductListingTitleTranslationError::InvalidResponse { .. }
        ));
        assert_eq!(1, fallback.prompts().len());
    }

    #[tokio::test]
    async fn should_never_reach_the_fallback_when_the_primary_fails_otherwise() {
        let source = || static_error("primary failure");
        let retryable = |kind| LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind,
                retry_after: None,
            },
            source: source(),
        };
        let failures = [
            LargeLanguageModelError::Timeout { source: source() },
            retryable(LargeLanguageModelRetryKind::RateLimited),
            retryable(LargeLanguageModelRetryKind::ServiceUnavailable),
            retryable(LargeLanguageModelRetryKind::Transient),
            LargeLanguageModelError::Authentication { source: source() },
            LargeLanguageModelError::InvalidRequest { source: source() },
            LargeLanguageModelError::Permanent { source: source() },
            LargeLanguageModelError::InvalidResponse { source: source() },
        ];
        for failure in failures {
            let kind = failure.kind();
            let primary = ScriptedModel::answering([Err(failure)]);
            let fallback = ScriptedModel::default();

            assert!(translate(&primary, &fallback).await.is_err(), "{kind}");
            assert!(fallback.prompts().is_empty(), "{kind} reached the fallback");
        }
    }

    #[tokio::test]
    async fn should_never_reach_the_fallback_when_the_primary_answers_unusably() {
        let primary = ScriptedModel::answering([Ok(serde_json::json!({"titles": {"en": " "}}))]);
        let fallback = ScriptedModel::default();

        let error = translate(&primary, &fallback).await.unwrap_err();

        assert!(matches!(
            error,
            ProductListingTitleTranslationError::InvalidResponse { .. }
        ));
        assert!(fallback.prompts().is_empty());
    }

    #[test]
    fn should_create_schema_for_each_requested_target_language() {
        let request = translation_request(
            &Title::from("Antiker Stuhl"),
            Language::De,
            &[Language::En, Language::Fr],
        );

        assert_eq!(
            Some("object"),
            request
                .response_json_schema
                .get("type")
                .and_then(serde_json::Value::as_str)
        );
        assert!(request.prompt.contains("Antiker Stuhl"));
        assert!(!request.prompt.contains("de, de"));
    }

    #[test]
    fn should_budget_room_for_a_maximal_dense_title_in_every_target() {
        let title = Title::from("宋".repeat(Title::MAX_CHARS).as_str());
        let targets = [
            Language::De,
            Language::En,
            Language::Fr,
            Language::Es,
            Language::It,
        ];
        let request = translation_request(&title, Language::Zh, &targets);

        // Two output tokens per source character and target exceeds the observed expansion.
        let worst_case = Title::MAX_CHARS * targets.len() * 2;
        assert!(
            usize::from(request.options.max_output_tokens) >= worst_case,
            "budget {} below the {worst_case}-token worst case",
            request.options.max_output_tokens
        );
    }
}
