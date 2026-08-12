use common::{
    error::boxed::{BoxError, box_error},
    logging::{
        GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
        log_llm_invocation,
    },
};
use futures::{StreamExt, stream};
use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
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
const VERTEX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub response_schema: serde_json::Value,
    pub options: GenerationOptions,
}

#[derive(Debug, thiserror::Error)]
pub enum LargeLanguageModelError {
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
            .timeout(VERTEX_REQUEST_TIMEOUT)
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
        let url = build_generate_content_url(&self.config);
        let provider_request = ProviderGenerateContentRequest::new(request, images);
        let operation = provider_request.operation;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(url)
            .bearer_auth(access_token.token)
            .json(&provider_request.body)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_status_error(status));
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

fn http_status_error(status: reqwest::StatusCode) -> LargeLanguageModelError {
    let source = std::io::Error::other(format!("Vertex AI returned HTTP {status}"));
    if status.as_u16() == 429 || status.is_server_error() || status.as_u16() == 408 {
        LargeLanguageModelError::Retryable {
            source: box_error(source),
        }
    } else {
        LargeLanguageModelError::Permanent {
            source: box_error(source),
        }
    }
}

struct ProviderGenerateContentRequest {
    operation: LlmOperation,
    body: GenerateContentBody,
}

impl ProviderGenerateContentRequest {
    fn new(request: StructuredGenerationRequest, images: Vec<FetchedImage>) -> Self {
        let mut contents = Vec::with_capacity(images.len() + 1);
        contents.push(ProviderContent::text(request.prompt));
        contents.extend(images.into_iter().map(ProviderContent::image));
        Self {
            operation: request.operation,
            body: GenerateContentBody {
                system_instruction: ProviderContent::text(request.system_instruction),
                contents,
                generation_config: GenerationConfig {
                    temperature: request.options.temperature,
                    max_output_tokens: request.options.max_output_tokens,
                    response_mime_type: "application/json",
                    response_schema: request.response_schema,
                },
            },
        }
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
    response_schema: serde_json::Value,
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
            LargeLanguageModelError::Retryable { .. }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::BAD_REQUEST),
            LargeLanguageModelError::Permanent { .. }
        ));
    }

    #[test]
    fn should_preserve_caller_generation_options() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::ProductEnhancedSearchDescriptionMatching,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_schema: serde_json::json!({"type": "OBJECT"}),
            options: GenerationOptions {
                temperature: 0.7,
                max_output_tokens: 512,
            },
        };

        let request = ProviderGenerateContentRequest::new(request, Vec::new());

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
            response_schema: serde_json::json!({"type": "OBJECT"}),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 1,
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
