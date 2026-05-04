use crate::types::ExtractedAttributes;
use async_trait::async_trait;
use llm::chat::ChatMessage;
use thiserror::Error;
use tracing::{debug, error};

/// Maximum total characters per Gemini batch to avoid context overflow.
const MAX_BATCH_CHARS: usize = 8_000;

#[derive(Debug, Error)]
pub enum ExtractionError {
    #[error("LLM request failed: {0}")]
    LlmError(#[from] llm::error::LLMError),
    #[error("Invalid LLM response: {0}")]
    InvalidResponse(String),
}

/// Extracts antique attributes from product texts via an LLM.
///
/// Accepts a slice of product texts (already sorted by length for optimal
/// batching), batches them into Gemini calls that stay within
/// [`MAX_BATCH_CHARS`], and returns one `Option<ExtractedAttributes>` per
/// input text in the same order.  A `None` entry means extraction failed for
/// that product; the handler will mark the corresponding SQS message as a
/// batch-item failure so it can be retried.
#[async_trait]
#[mockall::automock]
pub trait ExtractionService {
    async fn extract(&self, product_texts: &[String]) -> Vec<Option<ExtractedAttributes>>;
}

pub struct ExtractionServiceImpl {
    llm: Box<dyn llm::LLMProvider>,
}

impl ExtractionServiceImpl {
    pub fn new(api_key: &str) -> Self {
        let llm = llm::builder::LLMBuilder::new()
            .backend(llm::builder::LLMBackend::Google)
            .api_key(api_key)
            .model("gemini-2.5-flash-lite")
            .system(
                "You are an antiques attribute extractor. \
                Given a numbered list of antique product texts, return a JSON array \
                with exactly one object per product in the same order. \
                Use null for uncertain or missing values. \
                Respond ONLY with the JSON array—no other text.",
            )
            .build()
            .expect("shouldn't fail building LLM provider");
        Self { llm }
    }

    #[cfg(test)]
    pub fn new_with_provider(llm: Box<dyn llm::LLMProvider>) -> Self {
        Self { llm }
    }

    async fn extract_batch(
        &self,
        texts: &[&str],
    ) -> Result<Vec<Option<ExtractedAttributes>>, ExtractionError> {
        let numbered_texts = texts
            .iter()
            .enumerate()
            .map(|(i, t)| format!("[{}] {}", i + 1, t))
            .collect::<Vec<_>>()
            .join("\n");

        let user_message = format!(
            "Schema for each object:\n\
             {{\"y\":int|null,\"yMin\":int|null,\"yMax\":int|null,\
             \"auth\":\"ORIGINAL\"|\"LATER_COPY\"|\"REPRODUCTION\"|\"QUESTIONABLE\"|\"UNKNOWN\"|null,\
             \"cond\":\"EXCELLENT\"|\"GREAT\"|\"GOOD\"|\"FAIR\"|\"POOR\"|\"UNKNOWN\"|null,\
             \"prov\":\"COMPLETE\"|\"PARTIAL\"|\"CLAIMED\"|\"NONE\"|\"UNKNOWN\"|null,\
             \"rest\":\"MAJOR\"|\"MINOR\"|\"NONE\"|\"UNKNOWN\"|null,\
             \"nazi\":true|false|null}}\n\
             y=exact year, yMin/yMax=year range (only one form per product).\n\
             Century note: early=first third, mid=middle third, late=last third.\n\
             nazi=true if from/related to Nazi Germany or SA/SS (even pre-1933).\n\n\
             Extract attributes from these antique products:\n{numbered_texts}"
        );

        debug!(
            batchSize = texts.len(),
            "Requesting attribute extraction from Gemini."
        );

        let response = self
            .llm
            .chat(&[ChatMessage::user().content(&user_message).build()])
            .await?;

        let response_text = response.text().ok_or_else(|| {
            ExtractionError::InvalidResponse("Empty response from LLM".to_string())
        })?;

        parse_extraction_response(&response_text, texts.len())
    }
}

#[async_trait]
impl ExtractionService for ExtractionServiceImpl {
    async fn extract(&self, product_texts: &[String]) -> Vec<Option<ExtractedAttributes>> {
        let mut results = vec![None; product_texts.len()];

        for batch_indices in make_batches(product_texts) {
            let batch_texts: Vec<&str> = batch_indices
                .iter()
                .map(|&i| product_texts[i].as_str())
                .collect();

            match self.extract_batch(&batch_texts).await {
                Ok(extracted) => {
                    for (batch_pos, &original_idx) in batch_indices.iter().enumerate() {
                        results[original_idx] = extracted.get(batch_pos).copied().flatten();
                    }
                }
                Err(err) => {
                    error!(
                        error = %err,
                        batchSize = batch_indices.len(),
                        "Batch attribute extraction failed."
                    );
                    // All items in this batch remain None → will be retried.
                }
            }
        }

        results
    }
}

/// Splits text indices into batches that each stay within [`MAX_BATCH_CHARS`].
///
/// Assumes `texts` is already sorted by length (shortest first) so batches
/// pack efficiently.  Each single text that alone exceeds the limit is placed
/// in its own batch.
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

fn parse_extraction_response(
    response: &str,
    expected_count: usize,
) -> Result<Vec<Option<ExtractedAttributes>>, ExtractionError> {
    let cleaned: String = response.chars().skip_while(|c| c != &'[').collect();

    let items: Vec<serde_json::Value> = serde_json::from_str(&cleaned).map_err(|e| {
        ExtractionError::InvalidResponse(format!("Failed to parse JSON array: {e}"))
    })?;

    if items.len() != expected_count {
        return Err(ExtractionError::InvalidResponse(format!(
            "Expected {expected_count} result(s) but got {}",
            items.len()
        )));
    }

    Ok(items
        .into_iter()
        .map(|v| serde_json::from_value(v).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::{LLMProvider, chat::ChatMessage, error::LLMError};
    use rstest::rstest;

    // ---------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------

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

    // ---------------------------------------------------------------------------
    // parse_extraction_response tests
    // ---------------------------------------------------------------------------

    #[test]
    fn should_parse_valid_single_item_array_for_extraction_response() {
        let response = r#"[{"y":1845,"cond":"GOOD","nazi":false}]"#;
        let result = parse_extraction_response(response, 1).unwrap();
        assert_eq!(1, result.len());
        let attrs = result[0].unwrap();
        assert_eq!(Some(1845.into()), attrs.y);
        assert!(attrs.y_min.is_none());
        assert!(attrs.y_max.is_none());
        assert!(attrs.nazi == Some(false));
    }

    #[test]
    fn should_parse_valid_multi_item_array_for_extraction_response() {
        let response = r#"[{"y":1800},{"yMin":1900,"yMax":1933,"nazi":true}]"#;
        let result = parse_extraction_response(response, 2).unwrap();
        assert_eq!(2, result.len());
        assert_eq!(Some(1800.into()), result[0].unwrap().y);
        let second = result[1].unwrap();
        assert_eq!(Some(1900.into()), second.y_min);
        assert_eq!(Some(1933.into()), second.y_max);
        assert_eq!(Some(true), second.nazi);
    }

    #[test]
    fn should_parse_response_with_leading_think_tags_for_extraction_response() {
        let response = r#"<think>reasoning</think>[{"y":1750}]"#;
        let result = parse_extraction_response(response, 1).unwrap();
        assert_eq!(Some(1750.into()), result[0].unwrap().y);
    }

    #[test]
    fn should_return_none_entry_when_individual_object_has_invalid_enum_for_extraction_response() {
        let response = r#"[{"cond":"NOT_A_VALID_COND"},{"cond":"GOOD"}]"#;
        let result = parse_extraction_response(response, 2).unwrap();
        assert!(result[0].is_none());
        assert!(result[1].is_some());
    }

    #[test]
    fn should_fail_when_array_length_does_not_match_expected_count_for_extraction_response() {
        let response = r#"[{"y":1800},{"y":1900}]"#;
        let result = parse_extraction_response(response, 3);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 3"));
    }

    #[test]
    fn should_fail_when_response_is_not_valid_json_for_extraction_response() {
        let response = "this is not json";
        let result = parse_extraction_response(response, 1);
        assert!(result.is_err());
    }

    #[test]
    fn should_fail_when_response_is_empty_for_extraction_response() {
        let result = parse_extraction_response("", 1);
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------------
    // make_batches tests
    // ---------------------------------------------------------------------------

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
        // Each text is MAX_BATCH_CHARS / 2 + 1 chars, so two texts exceed the limit.
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

    // ---------------------------------------------------------------------------
    // ExtractionServiceImpl tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_attributes_when_llm_returns_valid_json_array_for_extraction_service() {
        let service = ExtractionServiceImpl::new_with_provider(Box::new(ReturningLlmProvider(
            r#"[{"y":1870,"cond":"GOOD","nazi":false}]"#.to_string(),
        )));

        let texts = vec!["Victorian oak chair circa 1870".to_string()];
        let results = service.extract(&texts).await;

        assert_eq!(1, results.len());
        let attrs = results[0].unwrap();
        assert_eq!(Some(1870.into()), attrs.y);
    }

    #[tokio::test]
    async fn should_return_all_nones_when_llm_call_fails_for_extraction_service() {
        let service = ExtractionServiceImpl::new_with_provider(Box::new(FailingLlmProvider));

        let texts = vec!["Antique chair".to_string(), "Old vase".to_string()];
        let results = service.extract(&texts).await;

        assert_eq!(2, results.len());
        assert!(results[0].is_none());
        assert!(results[1].is_none());
    }

    #[tokio::test]
    async fn should_return_empty_vec_when_input_is_empty_for_extraction_service() {
        let service = ExtractionServiceImpl::new_with_provider(Box::new(ReturningLlmProvider(
            "[]".to_string(),
        )));

        let results = service.extract(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn should_batch_large_inputs_and_return_correct_count_for_extraction_service() {
        // Use texts large enough to force batching.
        let long_text = "x".repeat(MAX_BATCH_CHARS / 2 + 1);
        let texts = vec![long_text.clone(), long_text.clone()];

        // The provider will be called twice (once per batch) and return one item each time.
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        struct CountingProvider(std::sync::Arc<std::sync::atomic::AtomicUsize>, String);

        #[async_trait::async_trait]
        impl llm::chat::ChatProvider for CountingProvider {
            async fn chat_with_tools(
                &self,
                _messages: &[ChatMessage],
                _tools: Option<&[llm::chat::Tool]>,
            ) -> Result<Box<dyn llm::chat::ChatResponse>, LLMError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Box::new(FakeChatResponse(Some(self.1.clone()))))
            }
        }

        #[async_trait::async_trait]
        impl llm::completion::CompletionProvider for CountingProvider {
            async fn complete(
                &self,
                _: &llm::completion::CompletionRequest,
            ) -> Result<llm::completion::CompletionResponse, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::embedding::EmbeddingProvider for CountingProvider {
            async fn embed(&self, _: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::stt::SpeechToTextProvider for CountingProvider {
            async fn transcribe(&self, _: Vec<u8>) -> Result<String, LLMError> {
                unimplemented!()
            }
        }

        #[async_trait::async_trait]
        impl llm::tts::TextToSpeechProvider for CountingProvider {}

        #[async_trait::async_trait]
        impl llm::models::ModelsProvider for CountingProvider {}

        impl LLMProvider for CountingProvider {}

        let service = ExtractionServiceImpl::new_with_provider(Box::new(CountingProvider(
            call_count_clone,
            r#"[{"y":1900}]"#.to_string(),
        )));

        let results = service.extract(&texts).await;

        assert_eq!(2, results.len());
        assert_eq!(2, call_count.load(std::sync::atomic::Ordering::SeqCst));
    }
}
