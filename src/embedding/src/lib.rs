use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, sync::Arc};
use tokio::sync::Mutex;
use url::Url;

const GEMINI_EMBEDDING_MODEL: &str = "gemini-embedding-2";
const QUERY_CACHE_CAPACITY: usize = 4096;
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
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError>;

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError>;
}

#[async_trait::async_trait]
impl<G> EmbeddingGenerator for Arc<G>
where
    G: EmbeddingGenerator + ?Sized,
{
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.as_ref()
            .embed_product(title, additional_text, image_url)
            .await
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.as_ref().embed_search_query(query).await
    }
}

/// Vertex AI Gemini Embedding implementation of [`EmbeddingGenerator`].
///
/// Callers supply semantic product or search fields. This adapter owns Google prompt
/// format, provider protocol, image retrieval, response validation, and query cache.
pub struct VertexAiEmbeddingGenerator {
    embed_content_url: String,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    credentials: AccessTokenCredentials,
    query_cache: Mutex<LruCache<EmbeddingText, EmbeddingVector>>,
}

impl VertexAiEmbeddingGenerator {
    pub fn new(config: VertexAiEmbeddingConfig, credentials: AccessTokenCredentials) -> Self {
        Self {
            embed_content_url: build_embed_content_url(&config),
            client: reqwest::Client::new(),
            image_fetcher: ImageFetcher::new(),
            credentials,
            query_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(QUERY_CACHE_CAPACITY).unwrap_or(NonZeroUsize::MIN),
            )),
        }
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        if let Some(vector) = self.query_cache.lock().await.get(query).cloned() {
            return Ok(vector);
        }

        let vector = self
            .request_embedding(EmbedContentRequest::for_search_query(query))
            .await?;
        self.query_cache
            .lock()
            .await
            .put(query.clone(), vector.clone());
        Ok(vector)
    }

    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        let image = match image_url {
            Some(image_url) => self.fetch_image(image_url).await,
            None => None,
        };
        self.request_embedding(EmbedContentRequest::for_product(
            title,
            additional_text,
            image,
        ))
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
        self.image_fetcher.fetch(image_url.as_url()).await
    }
}

#[async_trait::async_trait]
impl EmbeddingGenerator for VertexAiEmbeddingGenerator {
    async fn embed_product(
        &self,
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image_url: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embed_product(title, additional_text, image_url).await
    }

    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embed_search_query(query).await
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbedContentRequest {
    content: Content,
    output_dimensionality: usize,
}

impl EmbedContentRequest {
    fn for_search_query(query: &EmbeddingText) -> Self {
        Self::for_parts(vec![ContentPart::Text {
            text: format!("task: search result | query: {}", query.as_str()),
        }])
    }

    fn for_product(
        title: &EmbeddingText,
        additional_text: Option<&EmbeddingText>,
        image: Option<FetchedImage>,
    ) -> Self {
        let additional_text = additional_text.map(EmbeddingText::as_str).unwrap_or("none");
        let mut parts = vec![ContentPart::Text {
            text: format!("title: {} | text: {additional_text}", title.as_str()),
        }];
        if let Some(image) = image {
            parts.push(ContentPart::InlineData {
                inline_data: InlineData {
                    mime_type: image.mime_type(),
                    data: image.base64_data().to_owned(),
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
    fn should_serialize_search_query_vertex_request() -> Result<(), EmbeddingError> {
        let query =
            EmbedContentRequest::for_search_query(&EmbeddingText::new("vintage brass lamp")?);

        assert_eq!(
            serde_json::to_value(query).map_err(|_| EmbeddingError::InvalidResponse {
                reason: "query request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"task: search result | query: vintage brass lamp"}]},"outputDimensionality":768})
        );
        Ok(())
    }

    #[test]
    fn should_serialize_product_vertex_request_with_additional_text_or_none()
    -> Result<(), EmbeddingError> {
        let title = EmbeddingText::new("vintage brass lamp")?;
        let additional_text = EmbeddingText::new("adjustable arm")?;

        assert_eq!(
            serde_json::to_value(EmbedContentRequest::for_product(
                &title,
                Some(&additional_text),
                None,
            ))
            .map_err(|_| EmbeddingError::InvalidResponse {
                reason: "product request serialization failed"
            })?,
            serde_json::json!({"content":{"parts":[{"text":"title: vintage brass lamp | text: adjustable arm"}]},"outputDimensionality":768})
        );
        assert_eq!(
            serde_json::to_value(EmbedContentRequest::for_product(&title, None, None)).map_err(
                |_| EmbeddingError::InvalidResponse {
                    reason: "product request serialization failed"
                }
            )?,
            serde_json::json!({"content":{"parts":[{"text":"title: vintage brass lamp | text: none"}]},"outputDimensionality":768})
        );
        Ok(())
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
