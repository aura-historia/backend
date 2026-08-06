use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use google_cloud_auth::credentials::AccessTokenCredentials;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

const GEMINI_EMBEDDING_MODEL: &str = "gemini-embedding-2";
const QUERY_CACHE_CAPACITY: usize = 4096;
const IMAGE_FETCH_MAX_ATTEMPTS: usize = 5;
const IMAGE_FETCH_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
pub const EMBEDDING_DIMENSIONS: usize = 768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiEmbeddingConfig {
    project_id: String,
    location: String,
}

impl VertexAiEmbeddingConfig {
    pub fn new(project_id: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn location(&self) -> &str {
        &self.location
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding input is invalid: {reason}")]
    InvalidInput { reason: &'static str },
    #[error("embedding authentication failed")]
    AuthenticationFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("embedding request failed")]
    RequestFailed {
        #[source]
        source: reqwest::Error,
    },
    #[error("embedding provider returned HTTP {status}")]
    ApiFailure { status: reqwest::StatusCode },
    #[error("embedding provider response is invalid: {reason}")]
    InvalidResponse { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingText(String);

impl EmbeddingText {
    pub fn new(value: impl Into<String>) -> Result<Self, EmbeddingError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(EmbeddingError::InvalidInput {
                reason: "embedding text is empty",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingImageUrl(Url);

impl EmbeddingImageUrl {
    pub fn new(url: Url) -> Result<Self, EmbeddingError> {
        if matches!(url.scheme(), "http" | "https") {
            Ok(Self(url))
        } else {
            Err(EmbeddingError::InvalidInput {
                reason: "embedding image URL must use HTTP or HTTPS",
            })
        }
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingContent {
    text: EmbeddingText,
    image_url: Option<EmbeddingImageUrl>,
}

impl EmbeddingContent {
    pub fn new(text: EmbeddingText) -> Self {
        Self {
            text,
            image_url: None,
        }
    }

    pub fn with_image_url(mut self, image_url: EmbeddingImageUrl) -> Self {
        self.image_url = Some(image_url);
        self
    }

    pub fn text(&self) -> &EmbeddingText {
        &self.text
    }

    pub fn image_url(&self) -> Option<&EmbeddingImageUrl> {
        self.image_url.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingInput {
    Query(EmbeddingText),
    Content(EmbeddingContent),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingVector(Vec<f32>);

impl EmbeddingVector {
    pub fn try_new(mut values: Vec<f32>) -> Result<Self, EmbeddingError> {
        normalize_embedding(&mut values)?;
        Ok(Self(values))
    }

    pub fn values(&self) -> &[f32] {
        &self.0
    }

    pub fn into_values(self) -> Vec<f32> {
        self.0
    }
}

#[async_trait::async_trait]
pub trait EmbeddingGenerator: Send + Sync {
    async fn generate(&self, input: &EmbeddingInput) -> Result<EmbeddingVector, EmbeddingError>;
}

#[async_trait::async_trait]
impl<G> EmbeddingGenerator for Arc<G>
where
    G: EmbeddingGenerator + ?Sized,
{
    async fn generate(&self, input: &EmbeddingInput) -> Result<EmbeddingVector, EmbeddingError> {
        self.as_ref().generate(input).await
    }
}

/// Vertex AI Gemini Embedding implementation of [`EmbeddingGenerator`].
///
/// The caller owns semantic text construction. This adapter owns provider protocol,
/// image retrieval, response validation, and the bounded query cache.
pub struct VertexAiEmbeddingGenerator {
    embed_content_url: String,
    client: reqwest::Client,
    credentials: AccessTokenCredentials,
    query_cache: Mutex<LruCache<EmbeddingText, EmbeddingVector>>,
}

impl VertexAiEmbeddingGenerator {
    pub fn new(config: VertexAiEmbeddingConfig, credentials: AccessTokenCredentials) -> Self {
        Self {
            embed_content_url: build_embed_content_url(&config),
            client: reqwest::Client::new(),
            credentials,
            query_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(QUERY_CACHE_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            )),
        }
    }

    async fn generate_query(
        &self,
        text: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        if let Some(vector) = self.query_cache.lock().await.get(text).cloned() {
            return Ok(vector);
        }

        let vector = self
            .request_embedding(EmbedContentRequest::for_query(text))
            .await?;
        self.query_cache
            .lock()
            .await
            .put(text.clone(), vector.clone());
        Ok(vector)
    }

    async fn generate_content(
        &self,
        content: &EmbeddingContent,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let image = match content.image_url() {
            Some(image_url) => self.fetch_image(image_url).await,
            None => None,
        };
        self.request_embedding(EmbedContentRequest::for_content(content.text(), image))
            .await
    }

    async fn request_embedding(
        &self,
        request: EmbedContentRequest,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let access_token = self.credentials.access_token().await.map_err(|source| {
            EmbeddingError::AuthenticationFailed {
                source: Box::new(source),
            }
        })?;
        let response = self
            .client
            .post(&self.embed_content_url)
            .bearer_auth(access_token.token)
            .json(&request)
            .send()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?;

        if !response.status().is_success() {
            return Err(EmbeddingError::ApiFailure {
                status: response.status(),
            });
        }

        response
            .json::<EmbedContentResponse>()
            .await
            .map_err(|source| EmbeddingError::RequestFailed { source })?
            .into_embedding_vector()
    }

    async fn fetch_image(&self, image_url: &EmbeddingImageUrl) -> Option<FetchedImage> {
        for attempt in 1..=IMAGE_FETCH_MAX_ATTEMPTS {
            match self.fetch_image_once(image_url.as_url()).await {
                Ok(image) => return Some(image),
                Err(_) if attempt == IMAGE_FETCH_MAX_ATTEMPTS => return None,
                Err(_) => tokio::time::sleep(image_fetch_backoff(attempt)).await,
            }
        }
        None
    }

    async fn fetch_image_once(&self, url: &Url) -> Result<FetchedImage, ImageFetchError> {
        let response = self.client.get(url.as_str()).send().await?;
        if !response.status().is_success() {
            return Err(ImageFetchError::Response);
        }
        if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
            let content_type = content_type
                .to_str()
                .map_err(|_| ImageFetchError::InvalidContentType)?;
            if !content_type_can_be_supported_image(content_type) {
                return Err(ImageFetchError::InvalidContentType);
            }
        }
        let bytes = response.bytes().await?;
        let mime_type = supported_image_mime_type_from_bytes(&bytes)
            .ok_or(ImageFetchError::UnsupportedImage)?;
        Ok(FetchedImage {
            mime_type,
            base64_data: BASE64.encode(bytes),
        })
    }
}

#[async_trait::async_trait]
impl EmbeddingGenerator for VertexAiEmbeddingGenerator {
    async fn generate(&self, input: &EmbeddingInput) -> Result<EmbeddingVector, EmbeddingError> {
        match input {
            EmbeddingInput::Query(text) => self.generate_query(text).await,
            EmbeddingInput::Content(content) => self.generate_content(content).await,
        }
    }
}

fn build_embed_content_url(config: &VertexAiEmbeddingConfig) -> String {
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{GEMINI_EMBEDDING_MODEL}:embedContent",
        config.project_id(),
        config.location(),
    )
}

fn image_fetch_backoff(attempt: usize) -> Duration {
    let multiplier = 1_u32 << attempt.saturating_sub(1);
    IMAGE_FETCH_INITIAL_BACKOFF.saturating_mul(multiplier)
}

fn supported_image_mime_type_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if bytes.get(4..8) == Some(b"ftyp") {
        let major_brand = bytes.get(8..12)?;
        if matches!(major_brand, b"heic" | b"heix" | b"hevc" | b"hevx") {
            return Some("image/heic");
        }
        if matches!(major_brand, b"mif1" | b"msf1") {
            return Some("image/heif");
        }
    }
    None
}

fn content_type_can_be_supported_image(content_type: &str) -> bool {
    let mime_type = content_type.split(';').next().unwrap_or_default().trim();
    supported_image_mime_type_from_content_type(mime_type).is_some()
        || matches!(
            mime_type.to_ascii_lowercase().as_str(),
            "application/octet-stream" | "binary/octet-stream"
        )
        || mime_type.to_ascii_lowercase().starts_with("image/")
}

fn supported_image_mime_type_from_content_type(content_type: &str) -> Option<&'static str> {
    if matches!(content_type, "image/jpeg" | "image/jpg" | "image/pjpeg") {
        return Some("image/jpeg");
    }
    if matches!(content_type, "image/png" | "image/x-png") {
        return Some("image/png");
    }
    if content_type.eq_ignore_ascii_case("image/webp") {
        return Some("image/webp");
    }
    if content_type.eq_ignore_ascii_case("image/heic") {
        return Some("image/heic");
    }
    if content_type.eq_ignore_ascii_case("image/heif") {
        return Some("image/heif");
    }
    None
}

fn normalize_embedding(values: &mut [f32]) -> Result<(), EmbeddingError> {
    if values.len() != EMBEDDING_DIMENSIONS {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding has an unexpected dimension",
        });
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding contains a non-finite value",
        });
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err(EmbeddingError::InvalidResponse {
            reason: "embedding has zero norm",
        });
    }
    for value in values {
        *value /= norm;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum ImageFetchError {
    #[error("image request failed")]
    Request(#[from] reqwest::Error),
    #[error("image response is unsuccessful")]
    Response,
    #[error("image content type is unsupported")]
    InvalidContentType,
    #[error("image body is unsupported")]
    UnsupportedImage,
}

struct FetchedImage {
    mime_type: &'static str,
    base64_data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbedContentRequest {
    content: Content,
    output_dimensionality: usize,
}

impl EmbedContentRequest {
    fn for_query(text: &EmbeddingText) -> Self {
        Self::for_parts(vec![ContentPart::Text {
            text: format!("task: search result | query: {}", text.as_str()),
        }])
    }

    fn for_content(text: &EmbeddingText, image: Option<FetchedImage>) -> Self {
        let mut parts = vec![ContentPart::Text {
            text: text.as_str().to_owned(),
        }];
        if let Some(image) = image {
            parts.push(ContentPart::InlineData {
                inline_data: InlineData {
                    mime_type: image.mime_type,
                    data: image.base64_data,
                },
            });
        }
        Self::for_parts(parts)
    }

    fn for_parts(parts: Vec<ContentPart>) -> Self {
        Self {
            content: Content { parts },
            output_dimensionality: EMBEDDING_DIMENSIONS,
        }
    }
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    mime_type: &'static str,
    data: String,
}

#[derive(Debug, Deserialize)]
struct EmbedContentResponse {
    #[serde(default)]
    embedding: Option<ProviderEmbedding>,
    #[serde(default)]
    embeddings: Vec<ProviderEmbedding>,
}

impl EmbedContentResponse {
    fn into_embedding_vector(self) -> Result<EmbeddingVector, EmbeddingError> {
        let values = self
            .embedding
            .or_else(|| self.embeddings.into_iter().next())
            .ok_or(EmbeddingError::InvalidResponse {
                reason: "embedding is missing",
            })?
            .values;
        EmbeddingVector::try_new(values)
    }
}

#[derive(Debug, Deserialize)]
struct ProviderEmbedding {
    values: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_vertex_endpoint_for_regional_and_global_locations() {
        assert_eq!(
            build_embed_content_url(&VertexAiEmbeddingConfig::new("test-project", "eu")),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/test-project/locations/eu/publishers/google/models/gemini-embedding-2:embedContent"
        );
        assert_eq!(
            build_embed_content_url(&VertexAiEmbeddingConfig::new("test-project", "global")),
            "https://aiplatform.googleapis.com/v1/projects/test-project/locations/global/publishers/google/models/gemini-embedding-2:embedContent"
        );
    }

    #[test]
    fn should_serialize_query_and_multimodal_vertex_requests() -> Result<(), EmbeddingError> {
        let query = EmbedContentRequest::for_query(&EmbeddingText::new("vintage brass lamp")?);
        let content = EmbedContentRequest::for_content(
            &EmbeddingText::new("title: lamp | text: none")?,
            Some(FetchedImage {
                mime_type: "image/png",
                base64_data: "aW1hZ2U=".to_owned(),
            }),
        );

        assert_eq!(
            serde_json::to_value(query).map_err(|_| EmbeddingError::InvalidResponse {
                reason: "query request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"task: search result | query: vintage brass lamp"}]},"outputDimensionality":768})
        );
        assert_eq!(
            serde_json::to_value(content).map_err(|_| EmbeddingError::InvalidResponse {
                reason: "content request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"title: lamp | text: none"},{"inlineData":{"mimeType":"image/png","data":"aW1hZ2U="}}]},"outputDimensionality":768})
        );
        Ok(())
    }

    #[test]
    fn should_detect_supported_image_types_and_retry_with_exponential_backoff() {
        assert_eq!(
            Some("image/jpeg"),
            supported_image_mime_type_from_bytes(&[0xff, 0xd8, 0xff])
        );
        assert_eq!(
            Some("image/png"),
            supported_image_mime_type_from_bytes(b"\x89PNG\r\n\x1a\n")
        );
        assert_eq!(
            Some("image/webp"),
            supported_image_mime_type_from_bytes(b"RIFFxxxxWEBP")
        );
        assert_eq!(
            Some("image/heic"),
            supported_image_mime_type_from_bytes(b"xxxxftypheic")
        );
        assert_eq!(None, supported_image_mime_type_from_bytes(b"not an image"));
        assert_eq!(Duration::from_millis(100), image_fetch_backoff(1));
        assert_eq!(Duration::from_millis(1_600), image_fetch_backoff(5));
    }

    #[test]
    fn should_validate_and_normalize_both_vertex_response_shapes() -> Result<(), EmbeddingError> {
        for response in [
            serde_json::json!({"embedding":{"values":vec![2.0_f32; EMBEDDING_DIMENSIONS]}}),
            serde_json::json!({"embeddings":[{"values":vec![2.0_f32; EMBEDDING_DIMENSIONS]}]}),
        ] {
            let response =
                serde_json::from_value::<EmbedContentResponse>(response).map_err(|_| {
                    EmbeddingError::InvalidResponse {
                        reason: "response deserialization failed",
                    }
                })?;
            let vector = response.into_embedding_vector()?;
            let norm = vector
                .values()
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt();
            assert!((norm - 1.0).abs() < 0.000_01);
        }
        Ok(())
    }

    #[test]
    fn should_reject_invalid_embedding_vectors() {
        assert!(EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS - 1]).is_err());
        assert!(EmbeddingVector::try_new(vec![0.0; EMBEDDING_DIMENSIONS]).is_err());
        let mut non_finite = vec![1.0; EMBEDDING_DIMENSIONS];
        non_finite[0] = f32::NAN;
        assert!(EmbeddingVector::try_new(non_finite).is_err());
    }

    #[tokio::test]
    async fn should_keep_query_cache_bounded_and_promote_recent_entry() -> Result<(), EmbeddingError>
    {
        let mut cache = LruCache::new(NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN));
        let first = EmbeddingText::new("first")?;
        let second = EmbeddingText::new("second")?;
        let third = EmbeddingText::new("third")?;
        let vector = EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS])?;

        cache.put(first.clone(), vector.clone());
        cache.put(second.clone(), vector.clone());
        let _ = cache.get(&first);
        cache.put(third.clone(), vector);

        assert!(cache.get(&first).is_some());
        assert!(cache.get(&second).is_none());
        assert!(cache.get(&third).is_some());
        Ok(())
    }
}
