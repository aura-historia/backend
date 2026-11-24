//! Anthropic's [Message Batches API](https://platform.claude.com/docs/en/build-with-claude/batch-processing):
//! asynchronous structured generation at half the synchronous price.
//!
//! A batch is created once, processed by the provider within 24 hours and read back once it has
//! ended. Results arrive in any order and are matched to their requests by custom ID. Each result
//! is decoded exactly as a synchronous response is: only a complete `end_turn` answer that
//! deserializes into the caller's output type succeeds, and a refusal keeps its own kind.

use crate::anthropic::{MessagesRequest, MessagesResponse, http_status_error, log_model};
use crate::{
    AnthropicClaude, LargeLanguageModelError, LargeLanguageModelRetryAdvice,
    LargeLanguageModelRetryKind, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
    StructuredGenerationRequest, invalid_response_error, log_llm_invocation_failure,
    parse_retry_after, request_error,
};
use application::error::static_error;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::HashSet, time::Duration};

/// The provider's limit on requests per batch.
pub const MAX_BATCH_REQUESTS: usize = 100_000;
const BATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const BATCH_RESULTS_TIMEOUT: Duration = Duration::from_secs(300);
/// One result line holds one message; a longer line is not a result this client asked for.
const MAX_RESULT_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_ID_CHARS: usize = 64;
const MAX_BATCH_ID_CHARS: usize = 255;

/// A request's identity within its batch: 1 to 64 ASCII letters, digits, `-` or `_`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BatchCustomId(String);

impl BatchCustomId {
    pub fn new(value: impl Into<String>) -> Result<Self, LargeLanguageModelError> {
        let value = value.into();
        if is_identifier(&value, MAX_ID_CHARS) {
            Ok(Self(value))
        } else {
            Err(LargeLanguageModelError::InvalidRequest {
                source: static_error("batch custom ID must be 1 to 64 letters, digits, - or _"),
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The provider's identifier of a submitted batch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LargeLanguageModelBatchId(String);

impl LargeLanguageModelBatchId {
    /// Validated, because it is placed in a request path and stored by callers.
    pub fn new(value: impl Into<String>) -> Result<Self, LargeLanguageModelError> {
        let value = value.into();
        if is_identifier(&value, MAX_BATCH_ID_CHARS) {
            Ok(Self(value))
        } else {
            Err(invalid_response_error(std::io::Error::other(
                "batch ID is not a provider identifier",
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LargeLanguageModelBatchId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_identifier(value: &str, max_chars: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_chars
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// One structured generation request of a batch.
#[derive(Debug, Clone)]
pub struct BatchRequest {
    pub custom_id: BatchCustomId,
    pub request: StructuredGenerationRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchProcessingStatus {
    /// Requests are still processing or being canceled.
    InProgress,
    /// Every request has a result.
    Ended,
}

#[derive(Debug)]
pub struct BatchItemResult<Output> {
    pub custom_id: String,
    pub outcome: BatchItemOutcome<Output>,
}

#[derive(Debug)]
pub enum BatchItemOutcome<Output> {
    Succeeded(Output),
    /// The model answered unusably or declined ([`LargeLanguageModelError::is_refusal`]), or the
    /// provider could not process the request.
    Failed(LargeLanguageModelError),
    /// The batch expired or was canceled before the request reached the model; it was not billed.
    NotProcessed,
}

/// Structured generation through a provider's asynchronous batch interface.
#[async_trait::async_trait]
pub trait LargeLanguageModelBatches: Send + Sync {
    /// Creates one batch. Requests may not carry images, and custom IDs must be unique.
    async fn submit_batch(
        &self,
        operation: LlmOperation,
        requests: Vec<BatchRequest>,
    ) -> Result<LargeLanguageModelBatchId, LargeLanguageModelError>;

    async fn batch_status(
        &self,
        batch: &LargeLanguageModelBatchId,
    ) -> Result<BatchProcessingStatus, LargeLanguageModelError>;

    /// Reads every result of an ended batch and logs its outcome counts and token usage.
    async fn batch_results<Output>(
        &self,
        operation: LlmOperation,
        batch: &LargeLanguageModelBatchId,
    ) -> Result<Vec<BatchItemResult<Output>>, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send;
}

#[async_trait::async_trait]
impl LargeLanguageModelBatches for AnthropicClaude {
    async fn submit_batch(
        &self,
        operation: LlmOperation,
        requests: Vec<BatchRequest>,
    ) -> Result<LargeLanguageModelBatchId, LargeLanguageModelError> {
        let body = BatchCreateRequest::try_new(self, requests)?;
        let size = body.requests.len();
        let response = self
            .client
            .post(self.batches_url())
            .timeout(BATCH_REQUEST_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(request_error)?;
        let batch = read_batch(response).await?;
        let id = LargeLanguageModelBatchId::new(batch.id)?;
        tracing::info!(
            eventType = "LLM_BATCH",
            llmOperation = %operation,
            llmProvider = %LlmProvider::Anthropic,
            llmModel = %self.batch_model(),
            llmOutcome = "SUBMITTED",
            llmBatchId = %id,
            batchSize = size,
            "LLM batch submitted."
        );
        Ok(id)
    }

    async fn batch_status(
        &self,
        batch: &LargeLanguageModelBatchId,
    ) -> Result<BatchProcessingStatus, LargeLanguageModelError> {
        let response = self
            .client
            .get(format!("{}/{}", self.batches_url(), batch.as_str()))
            .timeout(BATCH_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(request_error)?;
        Ok(
            match read_batch(response).await?.processing_status.as_str() {
                "ended" => BatchProcessingStatus::Ended,
                "in_progress" | "canceling" => BatchProcessingStatus::InProgress,
                _ => {
                    return Err(invalid_response_error(std::io::Error::other(
                        "unknown batch processing status",
                    )));
                }
            },
        )
    }

    async fn batch_results<Output>(
        &self,
        operation: LlmOperation,
        batch: &LargeLanguageModelBatchId,
    ) -> Result<Vec<BatchItemResult<Output>>, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .get(format!("{}/{}/results", self.batches_url(), batch.as_str()))
            .timeout(BATCH_RESULTS_TIMEOUT)
            .send()
            .await
            .map_err(request_error)?;
        let response = successful(response)?;
        let lines = read_result_lines(response).await?;
        let mut summary = BatchSummary::default();
        let mut results = Vec::with_capacity(lines.len());
        for line in lines {
            // A line without its custom ID cannot be attributed, so it fails the whole read; a
            // result this client cannot decode fails only its own request.
            let line =
                serde_json::from_slice::<BatchResultLine>(&line).map_err(invalid_response_error)?;
            let outcome = match serde_json::from_value::<BatchResultBody>(line.result) {
                Ok(result) => self.item_outcome(operation, result, &mut summary, started_at),
                Err(source) => {
                    summary.invalid += 1;
                    BatchItemOutcome::Failed(invalid_response_error(source))
                }
            };
            results.push(BatchItemResult {
                custom_id: line.custom_id,
                outcome,
            });
        }
        summary.log(operation, self.batch_model(), batch, started_at.elapsed());
        Ok(results)
    }
}

impl AnthropicClaude {
    fn batches_url(&self) -> String {
        format!("{}/batches", self.messages_url)
    }

    fn batch_model(&self) -> LlmModel {
        log_model(self.config.model.as_str())
    }

    fn item_outcome<Output: DeserializeOwned>(
        &self,
        operation: LlmOperation,
        result: BatchResultBody,
        summary: &mut BatchSummary,
        started_at: std::time::Instant,
    ) -> BatchItemOutcome<Output> {
        match result {
            BatchResultBody::Succeeded { message } => {
                let metrics = message.usage_metrics();
                summary.add_usage(metrics);
                match message.into_output() {
                    Ok(output) => {
                        summary.succeeded += 1;
                        BatchItemOutcome::Succeeded(output)
                    }
                    Err(error) => {
                        if error.is_refusal() {
                            summary.refused += 1;
                        } else {
                            summary.invalid += 1;
                        }
                        if let Some(kind) = error.structured_response_failure_kind() {
                            log_llm_invocation_failure(
                                operation,
                                LlmProvider::Anthropic,
                                self.batch_model(),
                                started_at.elapsed(),
                                LlmInvocationMetrics {
                                    batch_size: Some(1),
                                    ..metrics
                                },
                                kind.as_str(),
                            );
                        }
                        BatchItemOutcome::Failed(error)
                    }
                }
            }
            BatchResultBody::Errored { error } => {
                summary.errored += 1;
                BatchItemOutcome::Failed(errored_result(&error.error.kind))
            }
            BatchResultBody::Canceled | BatchResultBody::Expired => {
                summary.not_processed += 1;
                BatchItemOutcome::NotProcessed
            }
        }
    }
}

#[derive(Serialize)]
struct BatchCreateRequest<'a> {
    requests: Vec<BatchCreateItem<'a>>,
}

#[derive(Serialize)]
struct BatchCreateItem<'a> {
    custom_id: String,
    params: MessagesRequest<'a>,
}

impl<'a> BatchCreateRequest<'a> {
    fn try_new(
        model: &'a AnthropicClaude,
        requests: Vec<BatchRequest>,
    ) -> Result<Self, LargeLanguageModelError> {
        if requests.is_empty() || requests.len() > MAX_BATCH_REQUESTS {
            return Err(LargeLanguageModelError::InvalidRequest {
                source: static_error("a batch holds 1 to 100,000 requests"),
            });
        }
        let mut custom_ids = HashSet::with_capacity(requests.len());
        let mut items = Vec::with_capacity(requests.len());
        for BatchRequest { custom_id, request } in requests {
            if !request.image_urls.is_empty() {
                return Err(LargeLanguageModelError::InvalidRequest {
                    source: static_error("batched requests cannot carry images"),
                });
            }
            if !custom_ids.insert(custom_id.0.clone()) {
                return Err(LargeLanguageModelError::InvalidRequest {
                    source: static_error("batch custom IDs must be unique"),
                });
            }
            items.push(BatchCreateItem {
                custom_id: custom_id.0,
                params: MessagesRequest::try_new(&model.config, request, Vec::new())?,
            });
        }
        Ok(Self { requests: items })
    }
}

#[derive(Deserialize)]
struct MessageBatch {
    id: String,
    processing_status: String,
}

#[derive(Deserialize)]
struct BatchResultLine {
    custom_id: String,
    result: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BatchResultBody {
    Succeeded { message: MessagesResponse },
    Errored { error: BatchErrorEnvelope },
    Canceled,
    Expired,
}

#[derive(Deserialize)]
struct BatchErrorEnvelope {
    error: BatchErrorDetail,
}

/// Only the provider's error type is read; its message can quote the request.
#[derive(Deserialize)]
struct BatchErrorDetail {
    #[serde(rename = "type")]
    kind: String,
}

fn errored_result(kind: &str) -> LargeLanguageModelError {
    let source = static_error("Anthropic could not process the batched request");
    let retryable = |kind| LargeLanguageModelError::Retryable {
        advice: LargeLanguageModelRetryAdvice {
            kind,
            retry_after: None,
        },
        source: static_error("Anthropic could not process the batched request"),
    };
    match kind {
        "invalid_request_error" | "request_too_large" | "not_found_error" => {
            LargeLanguageModelError::InvalidRequest { source }
        }
        "authentication_error" | "permission_error" => {
            LargeLanguageModelError::Authentication { source }
        }
        "rate_limit_error" => retryable(LargeLanguageModelRetryKind::RateLimited),
        "overloaded_error" => retryable(LargeLanguageModelRetryKind::ServiceUnavailable),
        _ => retryable(LargeLanguageModelRetryKind::Transient),
    }
}

fn successful(response: reqwest::Response) -> Result<reqwest::Response, LargeLanguageModelError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(parse_retry_after);
    Err(http_status_error(status, retry_after))
}

async fn read_batch(response: reqwest::Response) -> Result<MessageBatch, LargeLanguageModelError> {
    successful(response)?
        .json::<MessageBatch>()
        .await
        .map_err(|source| {
            if source.is_timeout() {
                request_error(source)
            } else {
                invalid_response_error(source)
            }
        })
}

/// Splits the JSONL body into its non-empty lines as they arrive.
async fn read_result_lines(
    mut response: reqwest::Response,
) -> Result<Vec<Vec<u8>>, LargeLanguageModelError> {
    let mut lines = Vec::new();
    let mut pending = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|source| {
        if source.is_timeout() {
            request_error(source)
        } else {
            invalid_response_error(source)
        }
    })? {
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line = pending.drain(..=end).collect::<Vec<_>>();
            push_line(&mut lines, &line[..end])?;
        }
        if pending.len() > MAX_RESULT_LINE_BYTES {
            return Err(oversized_line());
        }
    }
    push_line(&mut lines, &pending)?;
    Ok(lines)
}

fn push_line(lines: &mut Vec<Vec<u8>>, line: &[u8]) -> Result<(), LargeLanguageModelError> {
    let line = line.trim_ascii();
    if line.len() > MAX_RESULT_LINE_BYTES {
        return Err(oversized_line());
    }
    if !line.is_empty() {
        lines.push(line.to_vec());
    }
    Ok(())
}

fn oversized_line() -> LargeLanguageModelError {
    invalid_response_error(std::io::Error::other("batch result line is too long"))
}

/// Outcome counts and summed token usage of one batch's results.
#[derive(Debug, Default, PartialEq, Eq)]
struct BatchSummary {
    succeeded: usize,
    refused: usize,
    invalid: usize,
    errored: usize,
    not_processed: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_prompt_tokens: u64,
}

impl BatchSummary {
    fn add_usage(&mut self, metrics: LlmInvocationMetrics) {
        self.prompt_tokens += u64::from(metrics.prompt_tokens.unwrap_or_default());
        self.completion_tokens += u64::from(metrics.completion_tokens.unwrap_or_default());
        self.cached_prompt_tokens += u64::from(metrics.cached_prompt_tokens.unwrap_or_default());
    }

    fn size(&self) -> usize {
        self.succeeded + self.refused + self.invalid + self.errored + self.not_processed
    }

    fn log(
        &self,
        operation: LlmOperation,
        model: LlmModel,
        batch: &LargeLanguageModelBatchId,
        latency: Duration,
    ) {
        tracing::info!(
            eventType = "LLM_BATCH",
            llmOperation = %operation,
            llmProvider = %LlmProvider::Anthropic,
            llmModel = %model,
            llmOutcome = "ENDED",
            llmBatchId = %batch,
            latencyMs = u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
            batchSize = self.size(),
            succeededCount = self.succeeded,
            refusedCount = self.refused,
            invalidCount = self.invalid,
            erroredCount = self.errored,
            notProcessedCount = self.not_processed,
            promptTokens = self.prompt_tokens,
            completionTokens = self.completion_tokens,
            cachedPromptTokens = self.cached_prompt_tokens,
            "LLM batch results read."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnthropicConfig, GenerationOptions};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    const TEST_KEY: &str = "test-anthropic-key";

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct Output {
        matched: bool,
    }

    fn request(prompt: &str) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::ProductTitleTranslation,
            system_instruction: "Translate".to_owned(),
            prompt: prompt.to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({
                "type": "object",
                "properties": {"matched": {"type": "boolean"}},
                "required": ["matched"]
            }),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 256,
                request_timeout: Duration::from_secs(5),
            },
        }
    }

    fn batch_request(custom_id: &str, prompt: &str) -> BatchRequest {
        BatchRequest {
            custom_id: BatchCustomId::new(custom_id).unwrap(),
            request: request(prompt),
        }
    }

    struct Captured {
        head: String,
        body: Vec<u8>,
    }

    /// Answers one HTTP request with `status` and `body`, and returns what it received.
    async fn serve_once(status: u16, body: String) -> (AnthropicClaude, JoinHandle<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut model =
            AnthropicClaude::new(AnthropicConfig::new("claude-haiku-5-5"), TEST_KEY).unwrap();
        model.messages_url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (head_end, length) = loop {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .map_or(0, |length| length.parse::<usize>().unwrap());
                    break (end + 4, length);
                }
            };
            while bytes.len() < head_end + length {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                bytes.extend_from_slice(&buffer[..count]);
            }
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            Captured {
                head: String::from_utf8_lossy(&bytes[..head_end]).to_ascii_lowercase(),
                body: bytes[head_end..].to_vec(),
            }
        });
        (model, task)
    }

    fn message(stop_reason: &str, text: &str) -> serde_json::Value {
        serde_json::json!({
            "content": [{"type": "text", "text": text}],
            "stop_reason": stop_reason,
            "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 7}
        })
    }

    #[tokio::test]
    async fn should_submit_each_request_with_its_custom_id_and_model_parameters() {
        let (model, server) = serve_once(
            200,
            r#"{"id":"msgbatch_01Abc","processing_status":"in_progress"}"#.to_owned(),
        )
        .await;

        let id = model
            .submit_batch(
                LlmOperation::ProductTitleTranslation,
                vec![batch_request("g1", "first"), batch_request("g2", "second")],
            )
            .await
            .unwrap();

        assert_eq!("msgbatch_01Abc", id.as_str());
        let captured = server.await.unwrap();
        assert!(
            captured
                .head
                .starts_with("post /v1/messages/batches http/1.1")
        );
        assert!(
            captured
                .head
                .contains(&format!("x-api-key: {TEST_KEY}\r\n"))
        );
        let body: serde_json::Value = serde_json::from_slice(&captured.body).unwrap();
        let requests = body["requests"].as_array().unwrap();
        assert_eq!(2, requests.len());
        assert_eq!("g1", requests[0]["custom_id"]);
        assert_eq!("claude-haiku-5-5", requests[0]["params"]["model"]);
        assert_eq!(256, requests[0]["params"]["max_tokens"]);
        assert_eq!(
            "first",
            requests[0]["params"]["messages"][0]["content"][0]["text"]
        );
        assert_eq!(
            false,
            requests[0]["params"]["output_config"]["format"]["schema"]["additionalProperties"]
        );
        assert_eq!("g2", requests[1]["custom_id"]);
    }

    #[tokio::test]
    async fn should_reject_unsendable_batches_before_any_request() {
        let model =
            AnthropicClaude::new(AnthropicConfig::new("claude-haiku-5-5"), TEST_KEY).unwrap();
        let mut with_image = batch_request("g1", "image");
        with_image
            .request
            .image_urls
            .push(url::Url::parse("https://example.test/image.jpg").unwrap());

        for requests in [
            Vec::new(),
            vec![with_image],
            vec![batch_request("g1", "a"), batch_request("g1", "b")],
        ] {
            assert!(matches!(
                model
                    .submit_batch(LlmOperation::ProductTitleTranslation, requests)
                    .await,
                Err(LargeLanguageModelError::InvalidRequest { .. })
            ));
        }
        assert!(BatchCustomId::new("").is_err());
        assert!(BatchCustomId::new("a".repeat(65)).is_err());
        assert!(BatchCustomId::new("g/1").is_err());
        assert!(LargeLanguageModelBatchId::new("../messages").is_err());
    }

    #[tokio::test]
    async fn should_report_whether_a_batch_has_ended() {
        for (wire, expected) in [
            ("in_progress", BatchProcessingStatus::InProgress),
            ("canceling", BatchProcessingStatus::InProgress),
            ("ended", BatchProcessingStatus::Ended),
        ] {
            let (model, server) = serve_once(
                200,
                format!(r#"{{"id":"msgbatch_1","processing_status":"{wire}"}}"#),
            )
            .await;
            let id = LargeLanguageModelBatchId::new("msgbatch_1").unwrap();

            assert_eq!(expected, model.batch_status(&id).await.unwrap());
            assert!(
                server
                    .await
                    .unwrap()
                    .head
                    .starts_with("get /v1/messages/batches/msgbatch_1 http/1.1")
            );
        }
    }

    #[tokio::test]
    async fn should_classify_every_result_type_by_custom_id() {
        let lines = [
            serde_json::json!({"custom_id": "ok", "result": {"type": "succeeded",
                "message": message("end_turn", r#"{"matched":true}"#)}}),
            serde_json::json!({"custom_id": "refused", "result": {"type": "succeeded",
                "message": message("refusal", "")}}),
            serde_json::json!({"custom_id": "truncated", "result": {"type": "succeeded",
                "message": message("max_tokens", r#"{"matched""#)}}),
            serde_json::json!({"custom_id": "invalid", "result": {"type": "errored",
                "error": {"type": "error", "error": {"type": "invalid_request_error",
                "message": "SENTINEL request detail"}}}}),
            serde_json::json!({"custom_id": "overloaded", "result": {"type": "errored",
                "error": {"type": "error", "error": {"type": "overloaded_error", "message": ""}}}}),
            serde_json::json!({"custom_id": "expired", "result": {"type": "expired"}}),
            serde_json::json!({"custom_id": "canceled", "result": {"type": "canceled"}}),
            serde_json::json!({"custom_id": "unknown", "result": {"type": "paused"}}),
        ];
        let body = lines.map(|line| line.to_string()).join("\n") + "\n";
        let (model, server) = serve_once(200, body).await;
        let id = LargeLanguageModelBatchId::new("msgbatch_1").unwrap();

        let results = model
            .batch_results::<Output>(LlmOperation::ProductTitleTranslation, &id)
            .await
            .unwrap();

        assert!(
            server
                .await
                .unwrap()
                .head
                .starts_with("get /v1/messages/batches/msgbatch_1/results http/1.1")
        );
        let by_id = |custom_id: &str| {
            &results
                .iter()
                .find(|result| result.custom_id == custom_id)
                .unwrap()
                .outcome
        };
        assert!(matches!(
            by_id("ok"),
            BatchItemOutcome::Succeeded(Output { matched: true })
        ));
        assert!(matches!(by_id("refused"), BatchItemOutcome::Failed(error) if error.is_refusal()));
        assert!(matches!(
            by_id("truncated"),
            BatchItemOutcome::Failed(error) if error.structured_response_failure_kind()
                == Some(crate::StructuredResponseFailureKind::MaxTokens)
        ));
        assert!(matches!(
            by_id("invalid"),
            BatchItemOutcome::Failed(LargeLanguageModelError::InvalidRequest { source })
                if !source.to_string().contains("SENTINEL")
        ));
        assert!(matches!(
            by_id("overloaded"),
            BatchItemOutcome::Failed(error) if error.diagnostic_category() == "unavailable"
        ));
        assert!(matches!(by_id("expired"), BatchItemOutcome::NotProcessed));
        assert!(matches!(by_id("canceled"), BatchItemOutcome::NotProcessed));
        assert!(matches!(
            by_id("unknown"),
            BatchItemOutcome::Failed(LargeLanguageModelError::InvalidResponse { .. })
        ));
    }

    #[tokio::test]
    async fn should_fail_closed_on_an_unreadable_result_line_or_status() {
        let (model, _server) = serve_once(200, "{\"custom_id\": 1}\n".to_owned()).await;
        let id = LargeLanguageModelBatchId::new("msgbatch_1").unwrap();
        assert!(matches!(
            model
                .batch_results::<Output>(LlmOperation::ProductTitleTranslation, &id)
                .await,
            Err(LargeLanguageModelError::InvalidResponse { .. })
        ));

        let (model, _server) = serve_once(404, String::new()).await;
        assert!(matches!(
            model
                .batch_results::<Output>(LlmOperation::ProductTitleTranslation, &id)
                .await,
            Err(LargeLanguageModelError::Permanent { .. })
        ));

        let (model, _server) = serve_once(
            200,
            r#"{"id":"msgbatch_1","processing_status":"paused"}"#.to_owned(),
        )
        .await;
        assert!(model.batch_status(&id).await.is_err());
    }

    #[test]
    fn should_sum_usage_and_count_outcomes() {
        let mut summary = BatchSummary::default();
        for _ in 0..2 {
            summary.add_usage(LlmInvocationMetrics {
                prompt_tokens: Some(100),
                completion_tokens: Some(20),
                cached_prompt_tokens: Some(7),
                ..Default::default()
            });
        }
        summary.succeeded = 1;
        summary.refused = 1;
        summary.not_processed = 1;

        assert_eq!(3, summary.size());
        assert_eq!(
            (200, 40, 14),
            (
                summary.prompt_tokens,
                summary.completion_tokens,
                summary.cached_prompt_tokens
            )
        );
    }
}
