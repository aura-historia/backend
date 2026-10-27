use crate::claude_model::{ClaudeEffort, ClaudeModel, ClaudeThinking, log_model};
use crate::{
    BatchGenerationOptions, LargeLanguageModel, LargeLanguageModelError,
    LargeLanguageModelRetryAdvice, LargeLanguageModelRetryKind, LlmInvocationMetrics, LlmProvider,
    StructuredGenerationRequest, StructuredResponseError, deserialize_structured_output,
    fetch_images, fetch_images_for_requests, images_for_request, invalid_response_error,
    log_llm_invocation, log_llm_invocation_failure, parse_retry_after, request_error,
};
use application::error::{box_error, static_error};
use futures::{StreamExt, stream};
use image_fetcher::{FetchedImage, ImageFetcher};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};

const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
// Latest published API version; model releases and GA features do not change it.
// https://platform.claude.com/docs/en/api/versioning
const API_VERSION: &str = "2023-06-01";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Provider model selection supplied by the composition root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicConfig {
    model: ClaudeModel,
}

impl AnthropicConfig {
    /// Select a Claude model supporting native structured output, such as
    /// `claude-opus-5-5`, `claude-sonnet-5-5`, or `claude-haiku-5-5`.
    /// Pass a [`ClaudeModel`] to configure model-specific thinking and effort;
    /// strings preserve provider defaults.
    pub fn new(model: impl Into<ClaudeModel>) -> Self {
        Self {
            model: model.into(),
        }
    }
}

/// Structured generation through Anthropic's direct Messages API.
///
/// Requests use the current `output_config.format` JSON schema format. Object
/// schemas are closed with `additionalProperties: false`, as required by
/// Anthropic's [structured output API](https://platform.claude.com/docs/en/build-with-claude/structured-outputs).
/// Schemas with dynamic object keys are rejected before sending; other unsupported
/// schema features are rejected by the provider. Constraints are never removed.
///
/// Claude 5.5 uses provider default sampling, so [`crate::GenerationOptions::temperature`]
/// is not sent. JSON text is deserialized through the caller's output type;
/// thinking blocks are ignored and only complete `end_turn` results are accepted.
///
/// Images use the same bounded, optional image fetching as [`crate::VertexAiGemini`].
/// Batches deduplicate image fetches and preserve request order with bounded
/// concurrency. Provider requests are attempted once, with retry advice returned
/// to the caller; successful and invalid structured results emit invocation metrics.
pub struct AnthropicClaude {
    config: AnthropicConfig,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    messages_url: String,
}

impl AnthropicClaude {
    /// Create a reusable client with credentials supplied by the composition root.
    /// API key headers are marked sensitive and HTTP error bodies are not retained.
    pub fn new(
        config: AnthropicConfig,
        api_key: impl AsRef<str>,
    ) -> Result<Self, LargeLanguageModelError> {
        if config.model.as_str().trim().is_empty() || api_key.as_ref().trim().is_empty() {
            return Err(LargeLanguageModelError::InvalidRequest {
                source: static_error("Anthropic model and API key must be nonempty"),
            });
        }
        let mut api_key = HeaderValue::from_str(api_key.as_ref()).map_err(|source| {
            LargeLanguageModelError::InvalidRequest {
                source: box_error(source),
            }
        })?;
        api_key.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", api_key);
        headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .default_headers(headers)
            .build()
            .map_err(|source| LargeLanguageModelError::InvalidRequest {
                source: box_error(source),
            })?;
        Ok(Self {
            config,
            client,
            image_fetcher: ImageFetcher::new(),
            messages_url: MESSAGES_URL.to_owned(),
        })
    }

    async fn generate_with_images<Output>(
        &self,
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let operation = request.operation;
        let timeout = request.options.request_timeout;
        let body = MessagesRequest::try_new(&self.config, request, images)?;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(&self.messages_url)
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(parse_retry_after);
            return Err(http_status_error(status, retry_after));
        }
        let response = response
            .json::<MessagesResponse>()
            .await
            .map_err(|source| {
                if source.is_timeout() {
                    request_error(source)
                } else {
                    invalid_response_error(source)
                }
            })?;
        let metrics = response.usage_metrics();
        match response.into_output() {
            Ok(output) => {
                log_llm_invocation(
                    operation,
                    LlmProvider::Anthropic,
                    log_model(self.config.model.as_str()),
                    started_at.elapsed(),
                    metrics,
                );
                Ok(output)
            }
            Err(error) => {
                if let Some(kind) = error.structured_response_failure_kind() {
                    log_llm_invocation_failure(
                        operation,
                        LlmProvider::Anthropic,
                        log_model(self.config.model.as_str()),
                        started_at.elapsed(),
                        metrics,
                        kind.as_str(),
                    );
                }
                Err(error)
            }
        }
    }
}

#[async_trait::async_trait]
impl LargeLanguageModel for AnthropicClaude {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let images = fetch_images(&self.image_fetcher, &request.image_urls).await;
        self.generate_with_images(request, images).await
    }

    async fn generate_batch<Output>(
        &self,
        requests: Vec<StructuredGenerationRequest>,
        options: BatchGenerationOptions,
    ) -> Vec<Result<Output, LargeLanguageModelError>>
    where
        Output: DeserializeOwned + Send,
    {
        let images = Arc::new(fetch_images_for_requests(&self.image_fetcher, &requests).await);
        stream::iter(requests)
            .map(|request| {
                let images = Arc::clone(&images);
                async move {
                    let images = images_for_request(&request, images.as_ref());
                    self.generate_with_images(request, images).await
                }
            })
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    system: String,
    messages: [Message; 1],
    max_tokens: u16,
    output_config: OutputConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ClaudeThinking>,
}

impl<'a> MessagesRequest<'a> {
    fn try_new(
        config: &'a AnthropicConfig,
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Self, LargeLanguageModelError> {
        if request.options.max_output_tokens == 0
            || request.options.request_timeout.is_zero()
            || !request.response_json_schema.is_object()
        {
            return Err(LargeLanguageModelError::InvalidRequest {
                source: static_error("invalid Anthropic generation options or JSON schema"),
            });
        }
        let mut content = Vec::with_capacity(images.len() + 1);
        content.extend(images.into_iter().map(|image| InputContent::Image {
            source: ImageSource {
                kind: "base64",
                media_type: image.mime_type(),
                data: image.base64_data().to_owned(),
            },
        }));
        content.push(InputContent::Text {
            text: request.prompt,
        });
        Ok(Self {
            model: config.model.as_str(),
            system: request.system_instruction,
            messages: [Message {
                role: "user",
                content,
            }],
            max_tokens: request.options.max_output_tokens,
            output_config: OutputConfig {
                effort: config.model.effort(),
                format: JsonOutputFormat {
                    kind: "json_schema",
                    schema: normalize_response_schema(request.response_json_schema)?,
                },
            },
            thinking: config.model.thinking(),
        })
    }
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: Vec<InputContent>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputContent {
    Text { text: String },
    Image { source: ImageSource },
}

#[derive(Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    kind: &'static str,
    media_type: &'static str,
    data: String,
}

#[derive(Serialize)]
struct OutputConfig {
    format: JsonOutputFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<ClaudeEffort>,
}

#[derive(Serialize)]
struct JsonOutputFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    schema: serde_json::Value,
}

fn normalize_response_schema(
    mut schema: serde_json::Value,
) -> Result<serde_json::Value, LargeLanguageModelError> {
    let Some(object) = schema.as_object_mut() else {
        return Ok(schema);
    };
    object.remove("$schema");
    if object
        .get("additionalProperties")
        .is_some_and(|value| value != &serde_json::Value::Bool(false))
    {
        return Err(LargeLanguageModelError::InvalidRequest {
            source: static_error(
                "Anthropic structured output does not support dynamic object keys",
            ),
        });
    }
    let object_type = object.get("type").is_some_and(|value| {
        value.as_str() == Some("object")
            || value
                .as_array()
                .is_some_and(|types| types.iter().any(|kind| kind == "object"))
    });
    if object_type || object.contains_key("properties") {
        object.insert("additionalProperties".to_owned(), false.into());
    }
    for keyword in ["properties", "$defs", "definitions"] {
        if let Some(children) = object
            .get_mut(keyword)
            .and_then(serde_json::Value::as_object_mut)
        {
            for child in children.values_mut() {
                *child = normalize_response_schema(child.take())?;
            }
        }
    }
    if let Some(items) = object.get_mut("items") {
        *items = normalize_response_schema(items.take())?;
    }
    for keyword in ["anyOf", "allOf", "oneOf", "prefixItems"] {
        if let Some(children) = object
            .get_mut(keyword)
            .and_then(serde_json::Value::as_array_mut)
        {
            for child in children {
                *child = normalize_response_schema(child.take())?;
            }
        }
    }
    Ok(schema)
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<OutputContent>,
    stop_reason: String,
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutputContent {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

/// Token usage in the shape Anthropic reports it, also passed through by Claude Code.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Usage {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens_details: OutputTokenDetails,
}

#[derive(Debug, Default, Deserialize)]
struct OutputTokenDetails {
    thinking_tokens: Option<u32>,
}

impl Usage {
    /// Count cache writes and reads as prompt tokens, as Anthropic bills them as input.
    pub(crate) fn metrics(&self) -> LlmInvocationMetrics {
        let prompt_tokens = self.input_tokens.and_then(|tokens| {
            tokens
                .checked_add(self.cache_creation_input_tokens.unwrap_or_default())?
                .checked_add(self.cache_read_input_tokens.unwrap_or_default())
        });
        LlmInvocationMetrics {
            prompt_tokens,
            completion_tokens: self.output_tokens,
            total_tokens: prompt_tokens
                .zip(self.output_tokens)
                .and_then(|(input, output)| input.checked_add(output)),
            cached_prompt_tokens: self.cache_read_input_tokens,
            reasoning_tokens: self.output_tokens_details.thinking_tokens,
            ..Default::default()
        }
    }
}

impl MessagesResponse {
    fn usage_metrics(&self) -> LlmInvocationMetrics {
        self.usage.metrics()
    }

    fn into_output<Output: DeserializeOwned>(self) -> Result<Output, LargeLanguageModelError> {
        if self.stop_reason == "max_tokens" {
            return Err(invalid_response_error(
                StructuredResponseError::TruncatedMaxTokens,
            ));
        }
        if self.stop_reason != "end_turn" {
            return Err(invalid_response_error(
                StructuredResponseError::MissingContent,
            ));
        }
        let text = self
            .content
            .into_iter()
            .filter_map(|content| match content {
                OutputContent::Text { text } => Some(text),
                OutputContent::Other => None,
            })
            .collect::<String>();
        if text.trim().is_empty() {
            return Err(invalid_response_error(
                StructuredResponseError::MissingContent,
            ));
        }
        let input = serde_json::from_str::<serde_json::Value>(&text).map_err(|source| {
            invalid_response_error(StructuredResponseError::JsonSyntax { source })
        })?;
        deserialize_structured_output(input)
    }
}

fn http_status_error(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
) -> LargeLanguageModelError {
    let source = box_error(std::io::Error::other(format!(
        "Anthropic returned HTTP {status}"
    )));
    match status.as_u16() {
        401 | 403 => LargeLanguageModelError::Authentication { source },
        429 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::RateLimited,
                retry_after,
            },
            source,
        },
        503 | 529 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                retry_after,
            },
            source,
        },
        408 | 500..=599 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after,
            },
            source,
        },
        _ => LargeLanguageModelError::Permanent { source },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ClaudeHaiku55Thinking, ClaudeReducedThinkingEffort, ClaudeSonnet55Thinking,
        GenerationOptions, LlmModel, LlmOperation, StructuredResponseFailureKind,
    };
    use std::num::NonZeroUsize;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
    };

    const TEST_KEY: &str = "test-anthropic-key";
    const TEST_MODEL: &str = "claude-sonnet-5-5";

    #[derive(Debug, Deserialize, PartialEq, Eq, schemars::JsonSchema)]
    struct Output {
        matched: bool,
    }

    fn request(prompt: &str) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "Classify the URL".to_owned(),
            prompt: prompt.to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({
                "type": "object",
                "properties": {"matched": {"type": "boolean"}},
                "required": ["matched"]
            }),
            options: GenerationOptions {
                temperature: 0.2,
                max_output_tokens: 512,
                request_timeout: Duration::from_secs(5),
            },
        }
    }

    fn response(input: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "content": [
                {"type": "thinking", "thinking": "private reasoning"},
                {"type": "text", "text": input.to_string()}
            ],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 11,
                "output_tokens": 7,
                "cache_creation_input_tokens": 3,
                "cache_read_input_tokens": 5,
                "output_tokens_details": {"thinking_tokens": 2}
            }
        })
    }

    fn model(url: String) -> AnthropicClaude {
        let mut model = AnthropicClaude::new(AnthropicConfig::new(TEST_MODEL), TEST_KEY)
            .unwrap_or_else(|error| panic!("client should initialize: {error}"));
        model.messages_url = url;
        model
    }

    struct CapturedRequest {
        headers: String,
        body: serde_json::Value,
    }

    async fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        let mut bytes = Vec::new();
        let (header_end, content_length) = loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.expect("request should read");
            assert_ne!(count, 0, "request should contain complete headers");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .expect("JSON request should have content length")
                    .parse::<usize>()
                    .expect("content length should be a number");
                break (end + 4, length);
            }
        };
        while bytes.len() < header_end + content_length {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.expect("body should read");
            assert_ne!(count, 0, "request should contain complete body");
            bytes.extend_from_slice(&buffer[..count]);
        }
        CapturedRequest {
            headers: String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase(),
            body: serde_json::from_slice(&bytes[header_end..header_end + content_length])
                .expect("request should be JSON"),
        }
    }

    async fn serve_once(
        status: u16,
        headers: &str,
        body: String,
        delay: Duration,
    ) -> (String, JoinHandle<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local server should bind");
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let headers = headers.to_owned();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let request = read_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            tokio::time::sleep(delay).await;
            let _ = stream.write_all(body.as_bytes()).await;
            request
        });
        (url, task)
    }

    #[tokio::test]
    async fn should_send_authenticated_messages_with_native_structured_output_for_each_55_model() {
        for model_id in ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"] {
            let (url, server) = serve_once(
                200,
                "",
                response(serde_json::json!({"matched": true})).to_string(),
                Duration::ZERO,
            )
            .await;
            let mut model = model(url);
            model.config = AnthropicConfig::new(model_id);
            let output = model.generate::<Output>(request("a URL")).await.unwrap();
            assert_eq!(output, Output { matched: true });
            let captured = server.await.unwrap();
            assert!(captured.headers.starts_with("post /v1/messages http/1.1"));
            assert!(
                captured
                    .headers
                    .contains(&format!("x-api-key: {TEST_KEY}\r\n"))
            );
            assert!(
                captured
                    .headers
                    .contains("anthropic-version: 2023-06-01\r\n")
            );
            assert_eq!(captured.body["model"], model_id);
            assert_eq!(log_model(model_id).as_str(), model_id);
            assert_eq!(captured.body["system"], "Classify the URL");
            assert_eq!(captured.body["messages"][0]["role"], "user");
            assert_eq!(captured.body["messages"][0]["content"][0]["text"], "a URL");
            assert_eq!(captured.body["max_tokens"], 512);
            for absent in [
                "temperature",
                "top_p",
                "top_k",
                "tools",
                "tool_choice",
                "output_format",
                "thinking",
            ] {
                assert!(captured.body.get(absent).is_none(), "unexpected {absent}");
            }
            assert!(!captured.headers.contains("anthropic-beta:"));
            assert!(captured.body["output_config"].get("effort").is_none());
            assert_eq!(
                captured.body["output_config"]["format"],
                serde_json::json!({
                    "type": "json_schema",
                    "schema": {
                        "type": "object", "properties": {"matched": {"type": "boolean"}},
                        "required": ["matched"], "additionalProperties": false
                    }
                })
            );
        }
    }

    #[tokio::test]
    async fn should_send_model_specific_adaptive_effort_with_structured_output() {
        for (effort, wire) in [
            (ClaudeEffort::Low, "low"),
            (ClaudeEffort::Medium, "medium"),
            (ClaudeEffort::High, "high"),
            (ClaudeEffort::XHigh, "xhigh"),
            (ClaudeEffort::Max, "max"),
        ] {
            for (selected, model_id, thinking) in [
                (ClaudeModel::Opus55(effort), "claude-opus-5-5", None),
                (
                    ClaudeModel::Sonnet55(ClaudeSonnet55Thinking::Adaptive(effort)),
                    "claude-sonnet-5-5",
                    Some("adaptive"),
                ),
                (
                    ClaudeModel::Haiku55(ClaudeHaiku55Thinking::Adaptive(effort)),
                    "claude-haiku-5-5",
                    Some("adaptive"),
                ),
            ] {
                let (url, server) = serve_once(
                    200,
                    "",
                    response(serde_json::json!({"matched": true})).to_string(),
                    Duration::ZERO,
                )
                .await;
                let mut model = model(url);
                model.config = AnthropicConfig::new(selected);
                assert_eq!(
                    model.generate::<Output>(request("a URL")).await.unwrap(),
                    Output { matched: true }
                );
                let captured = server.await.unwrap();
                assert_eq!(captured.body["model"], model_id);
                assert_eq!(captured.body["output_config"]["effort"], wire);
                assert_eq!(
                    captured.body["output_config"]["format"]["type"],
                    "json_schema"
                );
                assert_eq!(captured.body["max_tokens"], 512);
                assert!(captured.body.get("temperature").is_none());
                match thinking {
                    Some(mode) => {
                        assert_eq!(captured.body["thinking"], serde_json::json!({"type": mode}))
                    }
                    None => assert!(captured.body.get("thinking").is_none()),
                }
            }
        }
    }

    #[test]
    fn should_serialize_reduced_thinking_with_only_supported_effort_levels() {
        for (effort, wire) in [
            (ClaudeReducedThinkingEffort::Low, "low"),
            (ClaudeReducedThinkingEffort::Medium, "medium"),
            (ClaudeReducedThinkingEffort::High, "high"),
        ] {
            for (model, model_id, mode) in [
                (
                    ClaudeModel::Sonnet55(ClaudeSonnet55Thinking::BetweenTools(effort)),
                    "claude-sonnet-5-5",
                    "between_tools",
                ),
                (
                    ClaudeModel::Haiku55(ClaudeHaiku55Thinking::Disabled(effort)),
                    "claude-haiku-5-5",
                    "disabled",
                ),
            ] {
                let config = AnthropicConfig::new(model);
                let body = serde_json::to_value(
                    MessagesRequest::try_new(&config, request(""), Vec::new()).unwrap(),
                )
                .unwrap();
                assert_eq!(body["model"], model_id);
                assert_eq!(body["output_config"]["effort"], wire);
                assert_eq!(body["thinking"], serde_json::json!({"type": mode}));
            }
        }
    }

    #[test]
    fn should_preserve_provider_defaults_for_string_selected_claude_models() {
        for model_id in [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-sonnet-5-5-unknown",
            "configured-model",
        ] {
            let owned_model_id = model_id.to_owned();
            for config in [
                AnthropicConfig::new(model_id),
                AnthropicConfig::new(owned_model_id.clone()),
                AnthropicConfig::new(&owned_model_id),
            ] {
                let body = serde_json::to_value(
                    MessagesRequest::try_new(&config, request(""), Vec::new()).unwrap(),
                )
                .unwrap();
                assert_eq!(body["model"], model_id);
                assert!(body.get("thinking").is_none());
                assert!(body["output_config"].get("effort").is_none());
            }
        }
    }

    #[test]
    fn should_close_nested_schemars_objects_and_preserve_references() {
        #[derive(schemars::JsonSchema)]
        #[allow(dead_code)]
        struct NestedOutput {
            results: Vec<Output>,
            optional: Option<Output>,
        }
        let config = AnthropicConfig::new(TEST_MODEL);
        let mut request = request("");
        request.response_json_schema =
            serde_json::to_value(schemars::schema_for!(NestedOutput)).unwrap();
        let original = request.response_json_schema.clone();
        let body = MessagesRequest::try_new(&config, request, Vec::new()).unwrap();
        let body = serde_json::to_value(body).unwrap();
        let schema = &body["output_config"]["format"]["schema"];
        assert!(schema.get("$schema").is_none());
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["$defs"]["Output"]["additionalProperties"], false);
        assert_eq!(schema["properties"], original["properties"]);
        assert_eq!(schema["required"], original["required"]);
        assert_eq!(
            schema["$defs"]["Output"]["properties"],
            original["$defs"]["Output"]["properties"]
        );
    }

    #[test]
    fn should_reject_dynamic_object_keys_and_preserve_constraints() {
        let config = AnthropicConfig::new(TEST_MODEL);
        for additional in [
            serde_json::json!(true),
            serde_json::json!({"type": "string"}),
        ] {
            let mut request = request("");
            request.response_json_schema = serde_json::json!({
                "type": "object", "properties": {
                    "map": {"type": "object", "additionalProperties": additional}
                }
            });
            assert!(matches!(
                MessagesRequest::try_new(&config, request, Vec::new()),
                Err(LargeLanguageModelError::InvalidRequest { .. })
            ));
        }
        let constrained = serde_json::json!({
            "type": "object", "properties": {
                "count": {"type": "integer", "minimum": 1},
                "constant": {"const": "value"},
                "anything": {"anyOf": [
                    {"type": "object", "properties": {"value": {"type": "string"}}},
                    {"type": "null"}
                ]}
            }
        });
        let normalized = normalize_response_schema(constrained.clone()).unwrap();
        assert_eq!(
            normalized["properties"]["count"],
            constrained["properties"]["count"]
        );
        assert_eq!(
            normalized["properties"]["constant"],
            constrained["properties"]["constant"]
        );
        assert_eq!(
            normalized["properties"]["anything"]["anyOf"][0]["additionalProperties"],
            false
        );
    }

    #[test]
    fn should_reject_invalid_credentials_and_generation_options() {
        for key in ["", " ", "secret\ninvalid-header"] {
            let error = AnthropicClaude::new(AnthropicConfig::new(TEST_MODEL), key)
                .err()
                .expect("invalid credential should be rejected");
            assert!(matches!(
                error,
                LargeLanguageModelError::InvalidRequest { .. }
            ));
            assert!(!format!("{error:?}").contains("secret"));
        }
        assert!(AnthropicClaude::new(AnthropicConfig::new(" "), TEST_KEY).is_err());
        let config = AnthropicConfig::new(TEST_MODEL);
        let mut no_tokens = request("");
        no_tokens.options.max_output_tokens = 0;
        assert!(MessagesRequest::try_new(&config, no_tokens, Vec::new()).is_err());
        let mut no_deadline = request("");
        no_deadline.options.request_timeout = Duration::ZERO;
        assert!(MessagesRequest::try_new(&config, no_deadline, Vec::new()).is_err());
    }

    #[tokio::test]
    async fn should_classify_http_failures_and_preserve_retry_after_without_payloads() {
        for (status, expected) in [
            (400, "permanent"),
            (401, "authentication"),
            (403, "authentication"),
            (404, "permanent"),
            (413, "permanent"),
            (429, "rate_limited"),
            (408, "transient"),
            (500, "transient"),
            (502, "transient"),
            (503, "service_unavailable"),
            (504, "transient"),
            (529, "service_unavailable"),
        ] {
            let (url, server) = serve_once(
                status,
                "Retry-After: 7\r\n",
                r#"{"error":{"type":"api_error","message":"private-provider-payload"}}"#.to_owned(),
                Duration::ZERO,
            )
            .await;
            let error = model(url)
                .generate::<Output>(request(""))
                .await
                .unwrap_err();
            assert_eq!(error.kind(), expected, "HTTP {status}");
            if let Some(advice) = error.retry_advice() {
                assert_eq!(advice.retry_after, Some(Duration::from_secs(7)));
            }
            let debug = format!("{error:?}");
            assert!(!debug.contains("private-provider-payload"));
            assert!(!debug.contains(TEST_KEY));
            assert!(std::error::Error::source(&error).is_some());
            server.await.unwrap();
        }
    }

    #[test]
    fn should_reject_truncated_refused_missing_and_invalid_outputs() {
        for reason in [
            "max_tokens",
            "refusal",
            "tool_use",
            "pause_turn",
            "model_context_window_exceeded",
        ] {
            let mut body = response(serde_json::json!({"matched": true}));
            body["stop_reason"] = reason.into();
            let response: MessagesResponse = serde_json::from_value(body).unwrap();
            let error = response.into_output::<Output>().unwrap_err();
            assert_eq!(
                error.structured_response_failure_kind(),
                Some(if reason == "max_tokens" {
                    StructuredResponseFailureKind::MaxTokens
                } else {
                    StructuredResponseFailureKind::MissingContent
                })
            );
        }
        for content in [
            serde_json::json!([]),
            serde_json::json!([{"type": "text", "text": " "}]),
            serde_json::json!([{"type": "thinking", "thinking": "{\"matched\":true}"}]),
            serde_json::json!([{"type": "tool_use", "name": "unexpected", "input": {"matched": true}}]),
            serde_json::json!([
                {"type": "text", "text": "{\"matched\":true}"},
                {"type": "text", "text": "{\"matched\":false}"}
            ]),
            serde_json::json!([{"type": "text", "text": "{\"matched\":"}]),
        ] {
            let response: MessagesResponse = serde_json::from_value(serde_json::json!({
                "content": content, "stop_reason": "end_turn"
            }))
            .unwrap();
            assert!(response.into_output::<Output>().is_err());
        }
    }

    #[test]
    fn should_parse_multipart_json_and_report_safe_json_syntax_feedback() {
        let valid: MessagesResponse = serde_json::from_value(serde_json::json!({
            "stop_reason": "end_turn",
            "content": [
                {"type": "text", "text": "{\"matched\":"},
                {"type": "thinking", "thinking": "private reasoning"},
                {"type": "text", "text": "true}"}
            ]
        }))
        .unwrap();
        assert_eq!(
            valid.into_output::<Output>().unwrap(),
            Output { matched: true }
        );
        let invalid: MessagesResponse = serde_json::from_value(serde_json::json!({
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "{\"matched\":private-response-content"}]
        }))
        .unwrap();
        let error = invalid.into_output::<Output>().unwrap_err();
        assert_eq!(
            error.structured_response_failure_kind(),
            Some(StructuredResponseFailureKind::InvalidJson)
        );
        assert_eq!(
            error.response_feedback_code(),
            Some("response_invalid_json")
        );
        assert!(!format!("{error:?}").contains("private-response-content"));
    }

    #[test]
    fn should_keep_structured_correction_feedback() {
        #[derive(Debug, Deserialize)]
        struct NestedOutput {
            result: Output,
        }
        for (input, code, feedback) in [
            (
                serde_json::json!({"result": {}}),
                "response_missing_required_field",
                "response_missing_required_field at result; missing_field=matched",
            ),
            (
                serde_json::json!({"result": {"matched": "private-value"}}),
                "response_schema_deserialization_failed",
                "response_schema_deserialization_failed at result.matched",
            ),
        ] {
            let response: MessagesResponse = serde_json::from_value(response(input)).unwrap();
            let error = response.into_output::<NestedOutput>().unwrap_err();
            assert_eq!(error.response_feedback_code(), Some(code));
            assert_eq!(
                error.response_correction_feedback().as_deref(),
                Some(feedback)
            );
            assert!(
                !error
                    .response_correction_feedback()
                    .unwrap()
                    .contains("private-value")
            );
        }
        let valid: MessagesResponse = serde_json::from_value(response(serde_json::json!({
            "result": {"matched": true}
        })))
        .unwrap();
        assert!(valid.into_output::<NestedOutput>().unwrap().result.matched);
    }

    #[test]
    fn should_count_cached_input_tokens_in_usage() {
        let response: MessagesResponse =
            serde_json::from_value(response(serde_json::json!({}))).unwrap();
        let metrics = response.usage_metrics();
        assert_eq!(metrics.prompt_tokens, Some(19));
        assert_eq!(metrics.completion_tokens, Some(7));
        assert_eq!(metrics.total_tokens, Some(26));
        assert_eq!(metrics.cached_prompt_tokens, Some(5));
        assert_eq!(metrics.reasoning_tokens, Some(2));
        assert_eq!(metrics.service_tier, None);
        assert_eq!(LlmProvider::Anthropic.as_str(), "ANTHROPIC");
        assert_eq!(log_model(TEST_MODEL).as_str(), TEST_MODEL);
        assert_eq!(log_model("unrecognized-model"), LlmModel::Configured);
    }

    #[tokio::test]
    async fn should_classify_deadline_and_invalid_response() {
        let (url, server) = serve_once(200, "", "{}".to_owned(), Duration::from_millis(150)).await;
        let mut short_request = request("");
        short_request.options.request_timeout = Duration::from_millis(20);
        let error = model(url)
            .generate::<Output>(short_request)
            .await
            .unwrap_err();
        assert!(matches!(error, LargeLanguageModelError::Timeout { .. }));
        server.await.unwrap();
        let (url, server) = serve_once(200, "", "not JSON".to_owned(), Duration::ZERO).await;
        let error = model(url)
            .generate::<Output>(request(""))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            LargeLanguageModelError::InvalidResponse { .. }
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn should_reject_redirects_without_forwarding_credentials() {
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!(
            "Location: http://{}/unexpected\r\n",
            destination.local_addr().unwrap()
        );
        let (url, server) = serve_once(302, &location, String::new(), Duration::ZERO).await;
        let error = model(url)
            .generate::<Output>(request(""))
            .await
            .unwrap_err();
        assert!(matches!(error, LargeLanguageModelError::Permanent { .. }));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), destination.accept())
                .await
                .is_err()
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn should_preserve_batch_order_and_individual_failures() {
        let (slow_url, slow_server) = serve_once(
            200,
            "",
            response(serde_json::json!({"matched": true})).to_string(),
            Duration::from_millis(50),
        )
        .await;
        let mut model = model(slow_url);
        model.config = AnthropicConfig::new(ClaudeModel::Sonnet55(
            ClaudeSonnet55Thinking::BetweenTools(ClaudeReducedThinkingEffort::Low),
        ));
        let mut invalid = request("invalid");
        invalid.options.max_output_tokens = 0;
        let results = model
            .generate_batch::<Output>(
                vec![request("slow"), invalid],
                BatchGenerationOptions::new(NonZeroUsize::new(2).unwrap()),
            )
            .await;
        let mut results = results.into_iter();
        assert_eq!(results.next().unwrap().unwrap(), Output { matched: true });
        assert!(matches!(
            results.next().unwrap(),
            Err(LargeLanguageModelError::InvalidRequest { .. })
        ));
        let captured = slow_server.await.unwrap();
        assert_eq!(captured.body["output_config"]["effort"], "low");
        assert_eq!(
            captured.body["thinking"],
            serde_json::json!({"type": "between_tools"})
        );
        assert!(
            model
                .generate_batch::<Output>(Vec::new(), BatchGenerationOptions::default())
                .await
                .is_empty()
        );
    }
}
