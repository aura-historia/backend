use common::error::boxed::{BoxError, box_error};
use google_cloud_auth::credentials::AccessTokenCredentials;
use search_filter_core::ProductSearch;
use search_filter_service::ports::{
    SearchFilterEmbeddingGenerationError, SearchFilterEmbeddingGenerator,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const GEMINI_EMBEDDING_MODEL: &str = "gemini-embedding-2";
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
pub enum VertexAiEmbeddingError {
    #[error("Vertex AI authentication failed")]
    AuthenticationFailed {
        #[source]
        source: BoxError,
    },
    #[error("Vertex AI request failed")]
    RequestFailed {
        #[source]
        source: reqwest::Error,
    },
    #[error("Vertex AI returned HTTP {status}")]
    ApiFailure { status: reqwest::StatusCode },
    #[error("Vertex AI embedding response is invalid: {reason}")]
    InvalidResponse { reason: &'static str },
}

/// Reusable direct Vertex AI embedding client.
///
/// The client has no knowledge of service ports, so another bounded context can
/// wrap `embed_query` with its own service-owned port when it needs embeddings.
pub struct VertexAiEmbeddingClient {
    embed_content_url: String,
    client: reqwest::Client,
    credentials: AccessTokenCredentials,
}

impl VertexAiEmbeddingClient {
    pub fn new(config: VertexAiEmbeddingConfig, credentials: AccessTokenCredentials) -> Self {
        Self {
            embed_content_url: build_embed_content_url(&config),
            client: reqwest::Client::new(),
            credentials,
        }
    }

    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>, VertexAiEmbeddingError> {
        let request = EmbedContentRequest::for_query(query);
        let access_token = self.credentials.access_token().await.map_err(|source| {
            VertexAiEmbeddingError::AuthenticationFailed {
                source: box_error(source),
            }
        })?;
        let response = self
            .client
            .post(&self.embed_content_url)
            .bearer_auth(access_token.token)
            .json(&request)
            .send()
            .await
            .map_err(|source| VertexAiEmbeddingError::RequestFailed { source })?;

        if !response.status().is_success() {
            return Err(VertexAiEmbeddingError::ApiFailure {
                status: response.status(),
            });
        }

        response
            .json::<EmbedContentResponse>()
            .await
            .map_err(|source| VertexAiEmbeddingError::RequestFailed { source })?
            .into_normalized_values()
    }
}

#[derive(Clone)]
pub struct VertexAiSearchFilterEmbeddingGenerator {
    client: Arc<VertexAiEmbeddingClient>,
}

impl VertexAiSearchFilterEmbeddingGenerator {
    pub fn new(client: Arc<VertexAiEmbeddingClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl SearchFilterEmbeddingGenerator for VertexAiSearchFilterEmbeddingGenerator {
    async fn generate(
        &self,
        search: &ProductSearch,
    ) -> Result<Option<Vec<f32>>, SearchFilterEmbeddingGenerationError> {
        let Some(query) = search_filter_query_text(search) else {
            return Ok(None);
        };

        self.client
            .embed_query(&query)
            .await
            .map(Some)
            .map_err(
                |source| SearchFilterEmbeddingGenerationError::GenerationFailed {
                    source: box_error(source),
                },
            )
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

fn search_filter_query_text(search: &ProductSearch) -> Option<String> {
    let mut parts = search
        .product_query
        .iter()
        .map(|query| query.as_ref().trim())
        .filter(|query| !query.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    if let Some(description) = &search.enhanced_search_description {
        let description = description.as_ref().trim();
        if !description.is_empty() {
            parts.push(description.to_owned());
        }
    }

    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EmbedContentRequest {
    content: Content,
    output_dimensionality: usize,
}

impl EmbedContentRequest {
    fn for_query(query: &str) -> Self {
        Self {
            content: Content {
                parts: vec![ContentPart::Text {
                    text: build_query_prompt(query),
                }],
            },
            output_dimensionality: EMBEDDING_DIMENSIONS,
        }
    }
}

fn build_query_prompt(query: &str) -> String {
    format!("task: search result | query: {query}")
}

#[derive(Debug, Serialize)]
struct Content {
    parts: Vec<ContentPart>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentPart {
    Text { text: String },
}

#[derive(Debug, Deserialize)]
struct EmbedContentResponse {
    #[serde(default)]
    embedding: Option<Embedding>,
    #[serde(default)]
    embeddings: Vec<Embedding>,
}

impl EmbedContentResponse {
    fn into_normalized_values(self) -> Result<Vec<f32>, VertexAiEmbeddingError> {
        let mut values = self
            .embedding
            .or_else(|| self.embeddings.into_iter().next())
            .ok_or(VertexAiEmbeddingError::InvalidResponse {
                reason: "embedding is missing",
            })?
            .values;
        normalize_embedding(&mut values)?;
        Ok(values)
    }
}

#[derive(Debug, Deserialize)]
struct Embedding {
    values: Vec<f32>,
}

fn normalize_embedding(values: &mut [f32]) -> Result<(), VertexAiEmbeddingError> {
    if values.len() != EMBEDDING_DIMENSIONS {
        return Err(VertexAiEmbeddingError::InvalidResponse {
            reason: "embedding has an unexpected dimension",
        });
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(VertexAiEmbeddingError::InvalidResponse {
            reason: "embedding contains a non-finite value",
        });
    }

    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err(VertexAiEmbeddingError::InvalidResponse {
            reason: "embedding has zero norm",
        });
    }

    for value in values {
        *value /= norm;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::currency::domain::Currency;
    use common::language::domain::Language;
    use product_core::product_search::EnhancedSearchDescription;

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
    fn should_build_query_prompt_and_vertex_request() -> Result<(), Box<dyn std::error::Error>> {
        let request = EmbedContentRequest::for_query("vintage brass lamp");

        assert_eq!(
            serde_json::to_value(request)?,
            serde_json::json!({
                "content": {
                    "parts": [{"text": "task: search result | query: vintage brass lamp"}]
                },
                "outputDimensionality": 768
            })
        );
        Ok(())
    }

    #[test]
    fn should_build_search_filter_query_text_from_product_query_and_enhanced_description()
    -> Result<(), Box<dyn std::error::Error>> {
        let search = ProductSearch::new(Language::En, Currency::Eur)
            .with_product_query("vintage lamp".try_into()?)
            .with_product_query("brass".try_into()?)
            .with_enhanced_search_description(EnhancedSearchDescription::from("table lighting"));

        assert_eq!(
            search_filter_query_text(&search).as_deref(),
            Some("vintage lamp\nbrass\ntable lighting")
        );
        Ok(())
    }

    #[test]
    fn should_not_embed_search_filter_without_text() {
        let search = ProductSearch::new(Language::En, Currency::Eur);

        assert_eq!(search_filter_query_text(&search), None);
    }

    #[test]
    fn should_validate_dimension_and_normalize_vertex_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let response: EmbedContentResponse = serde_json::from_value(serde_json::json!({
            "embedding": {"values": vec![2.0_f32; EMBEDDING_DIMENSIONS]}
        }))?;

        let values = response.into_normalized_values()?;
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();

        assert_eq!(values.len(), EMBEDDING_DIMENSIONS);
        assert!((norm - 1.0).abs() < 0.000_01);
        Ok(())
    }

    #[test]
    fn should_reject_vertex_response_with_wrong_dimension() -> Result<(), Box<dyn std::error::Error>>
    {
        let response: EmbedContentResponse = serde_json::from_value(serde_json::json!({
            "embedding": {"values": vec![1.0_f32; EMBEDDING_DIMENSIONS - 1]}
        }))?;

        let result = response.into_normalized_values();

        assert!(matches!(
            result,
            Err(VertexAiEmbeddingError::InvalidResponse {
                reason: "embedding has an unexpected dimension"
            })
        ));
        Ok(())
    }

    #[test]
    fn should_reject_zero_or_non_finite_vertex_response() {
        for values in [vec![0.0_f32; EMBEDDING_DIMENSIONS], {
            let mut values = vec![1.0_f32; EMBEDDING_DIMENSIONS];
            values[0] = f32::NAN;
            values
        }] {
            assert!(normalize_embedding(&mut values.clone()).is_err());
        }
    }
}
