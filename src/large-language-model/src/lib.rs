use application::error::{BoxError, box_error};
mod llm_logging;
use futures::{StreamExt, stream};
use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
pub use llm_logging::{
    GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
    log_llm_invocation,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};
use url::Url;

const MAX_IMAGES_PER_REQUEST: usize = 5;
const VERTEX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_CONCURRENT_REQUESTS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiConfig {
    project_id: String,
    location: String,
    model: String,
}

impl VertexAiConfig {
    pub fn new(
        project_id: impl Into<String>,
        location: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
            model: model.into(),
        }
    }

    fn project_id(&self) -> &str {
        &self.project_id
    }

    fn location(&self) -> &str {
        &self.location
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenerationOptions {
    pub temperature: f32,
    pub max_output_tokens: u16,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchGenerationOptions {
    pub max_concurrent_requests: NonZeroUsize,
}

impl BatchGenerationOptions {
    pub const fn new(max_concurrent_requests: NonZeroUsize) -> Self {
        Self {
            max_concurrent_requests,
        }
    }
}

impl Default for BatchGenerationOptions {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT_REQUESTS)
    }
}

#[derive(Debug, Clone)]
pub struct StructuredGenerationRequest {
    pub operation: LlmOperation,
    pub system_instruction: String,
    pub prompt: String,
    pub image_urls: Vec<Url>,
    pub response_json_schema: serde_json::Value,
    pub options: GenerationOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeLanguageModelRetryKind {
    RateLimited,
    ServiceUnavailable,
    Transient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeLanguageModelRetryAdvice {
    pub kind: LargeLanguageModelRetryKind,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, thiserror::Error)]
pub enum LargeLanguageModelError {
    #[error("large language model request configuration is invalid")]
    InvalidRequest {
        #[source]
        source: BoxError,
    },
    #[error("large language model authentication failed")]
    Authentication {
        #[source]
        source: BoxError,
    },
    #[error("large language model request timed out")]
    Timeout {
        #[source]
        source: BoxError,
    },
    #[error("large language model is temporarily unavailable")]
    Retryable {
        advice: LargeLanguageModelRetryAdvice,
        #[source]
        source: BoxError,
    },
    #[error("large language model rejected the request")]
    Permanent {
        #[source]
        source: BoxError,
    },
    #[error("large language model returned an invalid response")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

impl LargeLanguageModelError {
    pub fn retry_advice(&self) -> Option<LargeLanguageModelRetryAdvice> {
        match self {
            Self::Retryable { advice, .. } => Some(*advice),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "invalid_request",
            Self::Authentication { .. } => "authentication",
            Self::Timeout { .. } => "timeout",
            Self::Retryable { advice, .. } => match advice.kind {
                LargeLanguageModelRetryKind::RateLimited => "rate_limited",
                LargeLanguageModelRetryKind::ServiceUnavailable => "service_unavailable",
                LargeLanguageModelRetryKind::Transient => "transient",
            },
            Self::Permanent { .. } => "permanent",
            Self::InvalidResponse { .. } => "invalid_response",
        }
    }
}

#[async_trait::async_trait]
pub trait LargeLanguageModel: Send + Sync {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send;

    async fn generate_batch<Output>(
        &self,
        requests: Vec<StructuredGenerationRequest>,
        options: BatchGenerationOptions,
    ) -> Vec<Result<Output, LargeLanguageModelError>>
    where
        Output: DeserializeOwned + Send,
    {
        stream::iter(requests)
            .map(|request| self.generate(request))
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

pub struct VertexAiGemini {
    config: VertexAiConfig,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    credentials: AccessTokenCredentials,
}

impl VertexAiGemini {
    pub fn new(
        config: VertexAiConfig,
        credentials: AccessTokenCredentials,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(VERTEX_CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            config,
            client,
            image_fetcher: ImageFetcher::new(),
            credentials,
        })
    }
}

#[async_trait::async_trait]
impl LargeLanguageModel for VertexAiGemini {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let images = self.fetch_images(&request.image_urls).await;
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
        let images = Arc::new(self.fetch_images_for_requests(&requests).await);
        stream::iter(requests)
            .map(|request| {
                let images = Arc::clone(&images);
                async move {
                    let request_images = images_for_request(&request, images.as_ref());
                    self.generate_with_images(request, request_images).await
                }
            })
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

impl VertexAiGemini {
    async fn generate_with_images<Output>(
        &self,
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let access_token = self.credentials.access_token().await.map_err(|source| {
            LargeLanguageModelError::Authentication {
                source: box_error(source),
            }
        })?;
        let request_timeout = request.options.request_timeout;
        let url = build_generate_content_url(&self.config);
        let provider_request = ProviderGenerateContentRequest::try_new(request, images)?;
        let operation = provider_request.operation;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(url)
            .timeout(request_timeout)
            .bearer_auth(access_token.token)
            .json(&provider_request.body)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(parse_retry_after);
            let body = read_bounded_response_body(response).await;
            return Err(http_status_error_with_body(status, retry_after, &body));
        }
        let response = response
            .json::<GenerateContentResponse>()
            .await
            .map_err(invalid_response_error)?;
        log_llm_invocation(
            operation,
            LlmProvider::Google,
            log_model(self.config.model()),
            started_at.elapsed(),
            response.usage_metrics(),
        );
        response.into_output()
    }
}

impl VertexAiGemini {
    async fn fetch_images(&self, urls: &[Url]) -> Vec<FetchedImage> {
        let mut images = Vec::with_capacity(MAX_IMAGES_PER_REQUEST);
        for url in urls.iter().take(MAX_IMAGES_PER_REQUEST) {
            if let Some(image) = self.image_fetcher.fetch(url).await {
                images.push(image);
            }
        }
        images
    }

    async fn fetch_images_for_requests(
        &self,
        requests: &[StructuredGenerationRequest],
    ) -> HashMap<String, FetchedImage> {
        let mut unique_urls = Vec::new();
        let mut seen = HashSet::new();
        for request in requests {
            for url in request.image_urls.iter().take(MAX_IMAGES_PER_REQUEST) {
                if seen.insert(url.as_str()) {
                    unique_urls.push(url);
                }
            }
        }
        let mut images = HashMap::with_capacity(unique_urls.len());
        for url in unique_urls {
            if let Some(image) = self.image_fetcher.fetch(url).await {
                images.insert(url.as_str().to_owned(), image);
            }
        }
        images
    }
}

fn images_for_request(
    request: &StructuredGenerationRequest,
    images: &HashMap<String, FetchedImage>,
) -> Vec<FetchedImage> {
    request
        .image_urls
        .iter()
        .take(MAX_IMAGES_PER_REQUEST)
        .filter_map(|url| images.get(url.as_str()).cloned())
        .collect()
}

fn build_generate_content_url(config: &VertexAiConfig) -> String {
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
        config.project_id(),
        config.location(),
        config.model(),
    )
}

fn log_model(model: &str) -> LlmModel {
    match model {
        "gemini-3.1-flash-lite" => LlmModel::Gemini31FlashLite,
        _ => LlmModel::Configured,
    }
}

fn request_error(source: reqwest::Error) -> LargeLanguageModelError {
    if source.is_timeout() {
        LargeLanguageModelError::Timeout {
            source: box_error(source),
        }
    } else {
        LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after: None,
            },
            source: box_error(source),
        }
    }
}

fn invalid_response_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> LargeLanguageModelError {
    LargeLanguageModelError::InvalidResponse {
        source: box_error(source),
    }
}

async fn read_bounded_response_body(mut response: reqwest::Response) -> String {
    let mut body = Vec::with_capacity(MAX_PROVIDER_ERROR_BODY_BYTES.min(1024));
    while let Ok(Some(chunk)) = response.chunk().await {
        let remaining = MAX_PROVIDER_ERROR_BODY_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() >= MAX_PROVIDER_ERROR_BODY_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

fn parse_retry_after(value: &reqwest::header::HeaderValue) -> Option<Duration> {
    let value = value.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    date.duration_since(std::time::SystemTime::now()).ok()
}

fn provider_error_detail(body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .and_then(|error| {
            let code = error.get("code").and_then(serde_json::Value::as_i64);
            let status = error.get("status").and_then(serde_json::Value::as_str);
            let message = error.get("message").and_then(serde_json::Value::as_str);
            let detail = message.or(status)?;
            Some(match code {
                Some(code) => format!("code={code} message={detail}"),
                None => detail.to_owned(),
            })
        })
        .unwrap_or_else(|| "provider error body unavailable".to_owned());
    detail.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum VertexResponseJsonSchemaError {
    #[error("unsupported schema keyword {keyword} at {pointer}")]
    UnsupportedKeyword { pointer: String, keyword: String },
    #[error("invalid schema keyword {keyword} at {pointer}")]
    InvalidKeyword { pointer: String, keyword: String },
    #[error("$ref cannot have non-$ siblings at {pointer}")]
    RefWithSiblings { pointer: String },
}

fn normalize_vertex_response_json_schema(
    schema: serde_json::Value,
) -> Result<serde_json::Value, VertexResponseJsonSchemaError> {
    normalize_schema_node(schema, "#", true)
}

fn normalize_schema_node(
    mut node: serde_json::Value,
    pointer: &str,
    is_root: bool,
) -> Result<serde_json::Value, VertexResponseJsonSchemaError> {
    let Some(object) = node.as_object_mut() else {
        return Ok(node);
    };

    if is_root {
        object.remove("$schema");
    }

    if let Some(constant) = object.remove("const") {
        if object.contains_key("enum") {
            return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                pointer: pointer.to_owned(),
                keyword: "const with enum".to_owned(),
            });
        }
        object.insert("enum".to_owned(), serde_json::Value::Array(vec![constant]));
    }

    let has_ref = object.contains_key("$ref");
    if has_ref && object.keys().any(|key| !key.starts_with('$')) {
        return Err(VertexResponseJsonSchemaError::RefWithSiblings {
            pointer: pointer.to_owned(),
        });
    }

    for keyword in [
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "dependentSchemas",
        "contains",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "minProperties",
        "maxProperties",
        "multipleOf",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "contentEncoding",
        "contentMediaType",
    ] {
        if object.contains_key(keyword) {
            return Err(VertexResponseJsonSchemaError::UnsupportedKeyword {
                pointer: pointer.to_owned(),
                keyword: keyword.to_owned(),
            });
        }
    }

    for keyword in [
        "$comment",
        "default",
        "examples",
        "example",
        "deprecated",
        "readOnly",
        "writeOnly",
    ] {
        object.remove(keyword);
    }

    let known = [
        "$id",
        "$defs",
        "$ref",
        "$anchor",
        "type",
        "format",
        "title",
        "description",
        "enum",
        "items",
        "prefixItems",
        "minItems",
        "maxItems",
        "minimum",
        "maximum",
        "anyOf",
        "oneOf",
        "properties",
        "additionalProperties",
        "required",
        "propertyOrdering",
    ];
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(VertexResponseJsonSchemaError::UnsupportedKeyword {
                pointer: pointer.to_owned(),
                keyword: key.clone(),
            });
        }
    }

    let keys = object.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let child_pointer = format!("{pointer}/{}", escape_json_pointer(&key));
        match key.as_str() {
            "$defs" | "properties" => {
                let Some(map) = object
                    .get_mut(&key)
                    .and_then(serde_json::Value::as_object_mut)
                else {
                    return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                        pointer: child_pointer,
                        keyword: key,
                    });
                };
                let names = map.keys().cloned().collect::<Vec<_>>();
                for name in names {
                    let name_pointer = format!("{child_pointer}/{}", escape_json_pointer(&name));
                    let child = map.remove(&name).ok_or_else(|| {
                        VertexResponseJsonSchemaError::InvalidKeyword {
                            pointer: name_pointer.clone(),
                            keyword: key.clone(),
                        }
                    })?;
                    map.insert(name, normalize_schema_node(child, &name_pointer, false)?);
                }
            }
            "items" | "additionalProperties" => {
                let Some(child) = object.remove(&key) else {
                    continue;
                };
                if child.is_object() {
                    object.insert(key, normalize_schema_node(child, &child_pointer, false)?);
                } else {
                    object.insert(key, child);
                }
            }
            "prefixItems" | "anyOf" | "oneOf" => {
                let Some(values) = object
                    .get_mut(&key)
                    .and_then(serde_json::Value::as_array_mut)
                else {
                    return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                        pointer: child_pointer,
                        keyword: key,
                    });
                };
                for (index, child) in values.iter_mut().enumerate() {
                    let child_pointer = format!("{child_pointer}/{index}");
                    let normalized = normalize_schema_node(child.take(), &child_pointer, false)?;
                    *child = normalized;
                }
            }
            _ => {}
        }
    }
    Ok(node)
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
fn http_status_error(status: reqwest::StatusCode) -> LargeLanguageModelError {
    http_status_error_with_body(status, None, "")
}

fn http_status_error_with_body(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
    body: &str,
) -> LargeLanguageModelError {
    let detail = provider_error_detail(body);
    let source = std::io::Error::other(format!("Vertex AI returned HTTP {status}: {detail}"));
    match status.as_u16() {
        401 | 403 => LargeLanguageModelError::Authentication {
            source: box_error(source),
        },
        429 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::RateLimited,
                retry_after,
            },
            source: box_error(source),
        },
        503 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                retry_after,
            },
            source: box_error(source),
        },
        408 | 500 | 502 | 504 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after,
            },
            source: box_error(source),
        },
        status if status >= 500 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after,
            },
            source: box_error(source),
        },
        _ => LargeLanguageModelError::Permanent {
            source: box_error(source),
        },
    }
}

struct ProviderGenerateContentRequest {
    operation: LlmOperation,
    body: GenerateContentBody,
}

impl ProviderGenerateContentRequest {
    fn try_new(
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Self, LargeLanguageModelError> {
        let response_json_schema = normalize_vertex_response_json_schema(
            request.response_json_schema,
        )
        .map_err(|source| LargeLanguageModelError::InvalidRequest {
            source: box_error(source),
        })?;
        let mut contents = Vec::with_capacity(images.len() + 1);
        contents.push(ProviderContent::text(request.prompt));
        contents.extend(images.into_iter().map(ProviderContent::image));
        Ok(Self {
            operation: request.operation,
            body: GenerateContentBody {
                system_instruction: ProviderContent::text(request.system_instruction),
                contents,
                generation_config: GenerationConfig {
                    temperature: request.options.temperature,
                    max_output_tokens: request.options.max_output_tokens,
                    response_mime_type: "application/json",
                    response_json_schema,
                },
            },
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentBody {
    system_instruction: ProviderContent,
    contents: Vec<ProviderContent>,
    generation_config: GenerationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderContent {
    parts: Vec<ProviderPart>,
}

impl ProviderContent {
    fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ProviderPart::Text { text: text.into() }],
        }
    }

    fn image(image: FetchedImage) -> Self {
        Self {
            parts: vec![ProviderPart::InlineData {
                inline_data: ProviderInlineData {
                    mime_type: image.mime_type().to_owned(),
                    data: image.base64_data().to_owned(),
                },
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ProviderPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: ProviderInlineData,
    },
}

impl ProviderPart {
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text { text } => Some(text),
            Self::InlineData { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u16,
    response_mime_type: &'static str,
    response_json_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ProviderCandidate>,
    #[serde(default)]
    usage_metadata: ProviderUsageMetadata,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
    cached_content_token_count: Option<u32>,
    thoughts_token_count: Option<u32>,
}

impl GenerateContentResponse {
    fn usage_metrics(&self) -> LlmInvocationMetrics {
        LlmInvocationMetrics {
            service_tier: Some(GeminiServiceTier::Standard),
            prompt_tokens: self.usage_metadata.prompt_token_count,
            completion_tokens: self.usage_metadata.candidates_token_count,
            total_tokens: self.usage_metadata.total_token_count,
            cached_prompt_tokens: self.usage_metadata.cached_content_token_count,
            reasoning_tokens: self.usage_metadata.thoughts_token_count,
            ..Default::default()
        }
    }

    fn into_output<Output>(self) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned,
    {
        let text = self
            .candidates
            .into_iter()
            .find_map(|candidate| candidate.content)
            .and_then(|content| content.parts.into_iter().find_map(ProviderPart::into_text))
            .ok_or_else(|| {
                invalid_response_error(std::io::Error::other("Vertex AI response has no content"))
            })?;
        serde_json::from_str(&text).map_err(invalid_response_error)
    }
}

#[derive(Debug, Deserialize)]
struct ProviderCandidate {
    content: Option<ProviderContent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_regional_and_global_vertex_endpoints_for_configured_model() {
        let config = VertexAiConfig::new("project", "eu", "gemini-3.1-flash-lite");
        assert_eq!(
            build_generate_content_url(&config),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/project/locations/eu/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
        let config = VertexAiConfig::new("project", "global", "configured-model");
        assert_eq!(
            build_generate_content_url(&config),
            "https://aiplatform.googleapis.com/v1/projects/project/locations/global/publishers/google/models/configured-model:generateContent"
        );
    }

    #[test]
    fn should_classify_transient_and_permanent_vertex_statuses() {
        assert!(matches!(
            http_status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            LargeLanguageModelError::Retryable {
                advice: LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::RateLimited,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            LargeLanguageModelError::Retryable {
                advice: LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::BAD_REQUEST),
            LargeLanguageModelError::Permanent { .. }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::UNAUTHORIZED),
            LargeLanguageModelError::Authentication { .. }
        ));
    }

    #[test]
    fn should_preserve_caller_generation_options() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::ProductEnhancedSearchDescriptionMatching,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object"}),
            options: GenerationOptions {
                temperature: 0.7,
                max_output_tokens: 512,
                request_timeout: Duration::from_secs(60),
            },
        };

        let request = ProviderGenerateContentRequest::try_new(request, Vec::new())
            .unwrap_or_else(|error| panic!("request should normalize: {error}"));

        assert_eq!(0.7, request.body.generation_config.temperature);
        assert_eq!(512, request.body.generation_config.max_output_tokens);
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct CallerDefinedOutput {
        matched: bool,
    }

    struct OrderedTestLargeLanguageModel;

    #[async_trait::async_trait]
    impl LargeLanguageModel for OrderedTestLargeLanguageModel {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            if request.prompt == "slow" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            serde_json::from_value(serde_json::json!({"matched": request.prompt == "fast"}))
                .map_err(invalid_response_error)
        }
    }

    fn test_request(prompt: &str) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::ProductEnhancedSearchDescriptionMatching,
            system_instruction: "system".to_owned(),
            prompt: prompt.to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object"}),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 1,
                request_timeout: Duration::from_secs(60),
            },
        }
    }

    #[tokio::test]
    async fn should_preserve_batch_request_order_with_concurrency()
    -> Result<(), LargeLanguageModelError> {
        let results = OrderedTestLargeLanguageModel
            .generate_batch::<CallerDefinedOutput>(
                vec![test_request("slow"), test_request("fast")],
                BatchGenerationOptions::new(NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN)),
            )
            .await;

        assert_eq!(
            vec![
                CallerDefinedOutput { matched: false },
                CallerDefinedOutput { matched: true },
            ],
            results.into_iter().collect::<Result<Vec<_>, _>>()?
        );
        Ok(())
    }

    #[test]
    fn should_serialize_standard_json_schema_without_vertex_openapi_schema() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "matched": {"const": true}
                }
            }),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(60),
            },
        };
        let provider = ProviderGenerateContentRequest::try_new(request, Vec::new())
            .unwrap_or_else(|error| panic!("request should normalize: {error}"));
        let body = serde_json::to_value(provider.body)
            .unwrap_or_else(|error| panic!("provider body should serialize: {error}"));
        let generation_config = &body["generationConfig"];
        assert_eq!(generation_config["responseMimeType"], "application/json");
        assert_eq!(generation_config["responseJsonSchema"]["type"], "object");
        assert_eq!(
            generation_config["responseJsonSchema"]["properties"]["matched"]["enum"],
            serde_json::json!([true])
        );
        assert!(generation_config.get("responseSchema").is_none());
        assert!(
            generation_config["responseJsonSchema"]
                .get("$schema")
                .is_none()
        );
    }

    #[test]
    fn should_reject_unsupported_schema_keywords_before_http() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object", "allOf": []}),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(60),
            },
        };
        assert!(matches!(
            ProviderGenerateContentRequest::try_new(request, Vec::new()),
            Err(LargeLanguageModelError::InvalidRequest { .. })
        ));
    }

    #[allow(dead_code)]
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct TestObject {
        matched: bool,
    }

    #[allow(dead_code)]
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "mapping_type", rename_all = "snake_case")]
    enum TestTaggedResponse {
        State { state: String },
        Regex { pattern: String, state: String },
    }

    #[test]
    fn should_normalize_representative_schemars_json_schemas() {
        let object = serde_json::to_value(schemars::schema_for!(TestObject))
            .unwrap_or_else(|error| panic!("object schema should serialize: {error}"));
        let tagged = serde_json::to_value(schemars::schema_for!(TestTaggedResponse))
            .unwrap_or_else(|error| panic!("tagged schema should serialize: {error}"));
        assert!(normalize_vertex_response_json_schema(object).is_ok());
        assert!(normalize_vertex_response_json_schema(tagged).is_ok());
    }

    #[test]
    fn should_parse_retry_after_delta_and_http_date() {
        let delta = reqwest::header::HeaderValue::from_static("7");
        assert_eq!(parse_retry_after(&delta), Some(Duration::from_secs(7)));
        let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(2));
        let date = reqwest::header::HeaderValue::from_str(&date)
            .unwrap_or_else(|error| panic!("date header should be valid: {error}"));
        assert!(parse_retry_after(&date).is_some_and(|delay| delay <= Duration::from_secs(2)));
    }

    #[test]
    fn should_deserialize_caller_defined_output_type() -> Result<(), LargeLanguageModelError> {
        let response = GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent::text(r#"{"matched":true}"#)),
            }],
            usage_metadata: ProviderUsageMetadata::default(),
        };

        assert_eq!(
            response.into_output::<CallerDefinedOutput>()?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }
}
