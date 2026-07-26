use async_trait::async_trait;
use common::language::domain::Language;
use common::logging::{
    LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider, log_llm_invocation,
};
use llm::backends::google::GooglePlatform;
use llm::chat::ChatMessage;
use std::collections::HashMap;
use std::time::Instant;
use strum::IntoEnumIterator;
use thiserror::Error;
use tracing::{debug, warn};

/// Maximum total characters per Gemini batch to avoid context overflow.
const MAX_BATCH_CHARS: usize = 8_000;

/// Translation target languages: De, En, Fr, Es, It.
fn translation_targets() -> Vec<Language> {
    Language::iter()
        .filter(|l| l.is_translation_target())
        .collect()
}

#[derive(Debug, Error)]
pub enum TranslationError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
}

/// Translates antique product titles via an LLM.
#[async_trait]
#[mockall::automock]
pub trait TranslationService {
    async fn translate(
        &self,
        titles: &[String],
        source_language: Language,
    ) -> Vec<Option<HashMap<Language, String>>>;
}

pub struct TranslationServiceImpl {
    llm: Box<dyn llm::LLMProvider>,
}

impl TranslationServiceImpl {
    pub fn new(api_key: &str) -> Self {
        let llm = llm::builder::LLMBuilder::new()
            .backend(llm::builder::LLMBackend::Google)
            .google_platform(GooglePlatform::GeminiEnterpriseAgent {
                project_id: "aura-historia".to_owned(),
                region: Some("europe-west3".to_owned()),
            })
            .api_key(api_key)
            .model("gemini-2.5-flash-lite")
            .system("You are a translation assistant for antique product titles.")
            .build()
            .expect("shouldn't fail building LLM provider");
        Self { llm }
    }

    #[cfg(test)]
    pub fn new_with_provider(llm: Box<dyn llm::LLMProvider>) -> Self {
        Self { llm }
    }

    async fn translate_batch(
        &self,
        titles: &[&str],
        source_language: Language,
        target_languages: &[Language],
    ) -> Result<Vec<Option<HashMap<Language, String>>>, TranslationError> {
        let numbered_titles = titles
            .iter()
            .enumerate()
            .map(|(i, t)| format!("[{}] {}", i + 1, t))
            .collect::<Vec<_>>()
            .join("\n");

        let target_language_codes = target_languages
            .iter()
            .map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let user_message = format!(
            "Translate these antique product titles from {source} to all of these languages: {targets}.\n\
             Keep their order and return a JSON array where each item is an object mapping language codes to translations.\n\
             If translation is unclear, use null for that item. Respond ONLY with the one-line JSON array—no other text.\n\n\
             {numbered_titles}",
            source = source_language.as_str(),
            targets = target_language_codes,
            numbered_titles = numbered_titles
        );

        debug!(
            batchSize = titles.len(),
            sourceLang = source_language.as_str(),
            "Requesting title translation from Gemini."
        );

        let started_at = Instant::now();
        let response = self
            .llm
            .chat(&[ChatMessage::user().content(&user_message).build()])
            .await?;
        log_llm_invocation(
            LlmOperation::ProductTitleTranslation,
            LlmProvider::Google,
            LlmModel::Gemini25FlashLite,
            started_at.elapsed(),
            llm_metrics(response.usage(), Some(titles.len())),
        );

        let response_text = response.text().ok_or_else(|| {
            TranslationError::InvalidResponse("Empty response from LLM".to_string())
        })?;

        parse_translation_response(&response_text, titles.len(), target_languages)
    }
}

fn llm_metrics(usage: Option<llm::chat::Usage>, batch_size: Option<usize>) -> LlmInvocationMetrics {
    let Some(usage) = usage else {
        return LlmInvocationMetrics {
            batch_size,
            ..Default::default()
        };
    };

    LlmInvocationMetrics {
        batch_size,
        prompt_tokens: Some(usage.prompt_tokens),
        completion_tokens: Some(usage.completion_tokens),
        total_tokens: Some(usage.total_tokens),
        cached_prompt_tokens: usage.prompt_tokens_details.and_then(|d| d.cached_tokens),
        reasoning_tokens: usage
            .completion_tokens_details
            .and_then(|d| d.reasoning_tokens),
        ..Default::default()
    }
}

#[async_trait]
impl TranslationService for TranslationServiceImpl {
    async fn translate(
        &self,
        titles: &[String],
        source_language: Language,
    ) -> Vec<Option<HashMap<Language, String>>> {
        let target_languages: Vec<Language> = translation_targets()
            .into_iter()
            .filter(|l| *l != source_language)
            .collect();

        if target_languages.is_empty() {
            return vec![None; titles.len()];
        }

        let mut results = vec![None; titles.len()];

        for batch_indices in make_batches(titles) {
            let batch_titles: Vec<&str> =
                batch_indices.iter().map(|&i| titles[i].as_str()).collect();

            match self
                .translate_batch(&batch_titles, source_language, &target_languages)
                .await
            {
                Ok(translated) => {
                    for (batch_pos, &original_idx) in batch_indices.iter().enumerate() {
                        results[original_idx] = translated.get(batch_pos).cloned().flatten();
                    }
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        batchSize = batch_indices.len(),
                        "Batch title translation failed."
                    );
                }
            }
        }

        results
    }
}

fn make_batches(texts: &[String]) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut current_batch: Vec<usize> = Vec::new();
    let mut current_chars: usize = 0;

    for (i, text) in texts.iter().enumerate() {
        let text_len = text.len();
        if !current_batch.is_empty() && current_chars + text_len > MAX_BATCH_CHARS {
            batches.push(std::mem::take(&mut current_batch));
            current_chars = 0;
        }
        current_batch.push(i);
        current_chars += text_len;
    }
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    batches
}

fn parse_translation_response(
    response: &str,
    expected_count: usize,
    target_languages: &[Language],
) -> Result<Vec<Option<HashMap<Language, String>>>, TranslationError> {
    let cleaned: String = response
        .trim()
        .trim_start_matches("```json")
        .trim_end_matches("```")
        .to_string();

    let items: Vec<serde_json::Value> = serde_json::from_str(&cleaned).map_err(|e| {
        TranslationError::InvalidResponse(format!("Failed to parse JSON array: {e}"))
    })?;

    if items.len() != expected_count {
        return Err(TranslationError::InvalidResponse(format!(
            "Expected {expected_count} result(s) but got {}",
            items.len()
        )));
    }

    Ok(items
        .into_iter()
        .map(|v| {
            if v.is_null() {
                return None;
            }
            let obj = v.as_object()?;
            let mut translations = HashMap::new();
            for lang in target_languages {
                if let Some(serde_json::Value::String(s)) = obj.get(lang.as_str()) {
                    translations.insert(*lang, s.clone());
                }
            }
            if translations.is_empty() {
                None
            } else {
                Some(translations)
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
    use rstest::rstest;

    struct FakeChatResponse(Option<String>);

    impl std::fmt::Display for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0.as_deref().unwrap_or(""))
        }
    }

    impl std::fmt::Debug for FakeChatResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FakeChatResponse({:?})", self.0)
        }
    }

    impl llm::chat::ChatResponse for FakeChatResponse {
        fn text(&self) -> Option<String> {
            self.0.clone()
        }

        fn tool_calls(&self) -> Option<Vec<llm::ToolCall>> {
            None
        }
    }

    struct ReturningLlmProvider(String);

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for ReturningLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            Ok(Box::new(FakeChatResponse(Some(self.0.clone()))))
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for ReturningLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for ReturningLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for ReturningLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for ReturningLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for ReturningLlmProvider {}

    impl LLMProvider for ReturningLlmProvider {}

    struct FailingLlmProvider;

    #[async_trait::async_trait]
    impl llm::chat::ChatProvider for FailingLlmProvider {
        async fn chat_with_tools(
            &self,
            _messages: &[ChatMessage],
            _tools: Option<&[llm::chat::Tool]>,
        ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
            Err(LLMError::ProviderError("network error".to_string()))
        }
    }

    #[async_trait::async_trait]
    impl llm::completion::CompletionProvider for FailingLlmProvider {
        async fn complete(
            &self,
            _req: &llm::completion::CompletionRequest,
        ) -> Result<llm::completion::CompletionResponse, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::embedding::EmbeddingProvider for FailingLlmProvider {
        async fn embed(&self, _input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::stt::SpeechToTextProvider for FailingLlmProvider {
        async fn transcribe(&self, _audio: Vec<u8>) -> Result<String, LLMError> {
            unimplemented!()
        }
    }

    #[async_trait::async_trait]
    impl llm::tts::TextToSpeechProvider for FailingLlmProvider {}

    #[async_trait::async_trait]
    impl llm::models::ModelsProvider for FailingLlmProvider {}

    impl LLMProvider for FailingLlmProvider {}

    #[test]
    fn should_parse_valid_single_item_array_for_translation_response() {
        let target_langs = vec![Language::En, Language::Fr];
        let response = r#"[{"en":"Antique oak chair","fr":"Chaise en chêne ancienne"}]"#;
        let result = parse_translation_response(response, 1, &target_langs).unwrap();
        assert_eq!(1, result.len());
        let map = result[0].as_ref().unwrap();
        assert_eq!(map.get(&Language::En).unwrap(), "Antique oak chair");
        assert_eq!(map.get(&Language::Fr).unwrap(), "Chaise en chêne ancienne");
    }

    #[test]
    fn should_parse_valid_multi_item_array_for_translation_response() {
        let target_langs = vec![Language::En, Language::Fr];
        let response = r#"[{"en":"Chair","fr":"Chaise"},{"en":"Table","fr":"Table"}]"#;
        let result = parse_translation_response(response, 2, &target_langs).unwrap();
        assert_eq!(2, result.len());
        assert_eq!(
            result[0].as_ref().unwrap().get(&Language::En).unwrap(),
            "Chair"
        );
        assert_eq!(
            result[1].as_ref().unwrap().get(&Language::En).unwrap(),
            "Table"
        );
    }

    #[test]
    fn should_parse_response_with_markdown_json_translation_response() {
        let target_langs = vec![Language::En];
        let response = r#"```json[{"en":"Antique chair"}]```"#;
        let result = parse_translation_response(response, 1, &target_langs).unwrap();
        assert_eq!(
            result[0].as_ref().unwrap().get(&Language::En).unwrap(),
            "Antique chair"
        );
    }

    #[test]
    fn should_return_none_entry_when_item_is_null_for_translation_response() {
        let target_langs = vec![Language::En, Language::Fr];
        let response = r#"[null,{"en":"Chair","fr":"Chaise"}]"#;
        let result = parse_translation_response(response, 2, &target_langs).unwrap();
        assert!(result[0].is_none());
        assert!(result[1].is_some());
    }

    #[test]
    fn should_fail_when_array_length_does_not_match_expected_count_for_translation_response() {
        let target_langs = vec![Language::En];
        let response = r#"[{"en":"Chair"},{"en":"Table"}]"#;
        let result = parse_translation_response(response, 3, &target_langs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 3"));
    }

    #[test]
    fn should_fail_when_response_is_not_valid_json_for_translation_response() {
        let target_langs = vec![Language::En];
        let response = "this is not json";
        let result = parse_translation_response(response, 1, &target_langs);
        assert!(result.is_err());
    }

    #[rstest]
    #[case(0, 0)]
    #[case(1, 1)]
    #[case(3, 1)]
    fn should_produce_correct_batch_count_for_small_inputs(
        #[case] text_count: usize,
        #[case] expected_batches: usize,
    ) {
        let texts: Vec<String> = (0..text_count).map(|i| format!("item {i}")).collect();
        assert_eq!(expected_batches, make_batches(&texts).len());
    }

    #[test]
    fn should_split_into_multiple_batches_when_chars_exceed_limit() {
        let long_text = "x".repeat(MAX_BATCH_CHARS / 2 + 1);
        let texts = vec![long_text.clone(), long_text.clone(), long_text.clone()];
        let batches = make_batches(&texts);
        assert_eq!(3, batches.len());
        for batch in &batches {
            assert_eq!(1, batch.len());
        }
    }

    #[test]
    fn should_preserve_all_indices_across_batches() {
        let texts: Vec<String> = (0..10).map(|i| format!("antique {i}")).collect();
        let batches = make_batches(&texts);
        let all_indices: Vec<usize> = batches.into_iter().flatten().collect();
        let mut sorted = all_indices.clone();
        sorted.sort_unstable();
        assert_eq!((0..10).collect::<Vec<_>>(), sorted);
    }

    #[tokio::test]
    async fn should_return_translations_when_llm_returns_valid_json_array_for_translation_service()
    {
        let service = TranslationServiceImpl::new_with_provider(Box::new(ReturningLlmProvider(
            r#"[{"en":"Antique oak chair","fr":"Chaise en chêne ancienne","es":"Silla de roble antiguo","it":"Sedia in rovere antico"}]"#.to_string(),
        )));

        let titles = vec!["Antiker Eichenstuhl".to_string()];
        let results = service.translate(&titles, Language::De).await;

        assert_eq!(1, results.len());
        let map = results[0].as_ref().unwrap();
        assert_eq!(map.get(&Language::En).unwrap(), "Antique oak chair");
    }

    #[tokio::test]
    async fn should_return_all_nones_when_llm_call_fails_for_translation_service() {
        let service = TranslationServiceImpl::new_with_provider(Box::new(FailingLlmProvider));

        let titles = vec!["Antike Vase".to_string(), "Alter Stuhl".to_string()];
        let results = service.translate(&titles, Language::De).await;

        assert_eq!(2, results.len());
        assert!(results[0].is_none());
        assert!(results[1].is_none());
    }

    #[tokio::test]
    async fn should_return_empty_vec_when_input_is_empty_for_translation_service() {
        let service = TranslationServiceImpl::new_with_provider(Box::new(ReturningLlmProvider(
            "[]".to_string(),
        )));

        let results = service.translate(&[], Language::De).await;
        assert!(results.is_empty());
    }
}
