mod vertex;

pub use vertex::VertexAiEmbeddingGenerator;

use std::{sync::Arc, time::Duration};
use url::Url;

pub const EMBEDDING_DIMENSIONS: usize = 768;

/// Upper bound for one uncached search-query embedding, from credential acquisition to the
/// decoded response body.
///
/// Query embeddings serve interactive requests. The bound stays well below the shortest public
/// request budget so callers can still answer without the embedding, for example by falling
/// back to lexical search on an initial search page.
pub const SEARCH_QUERY_EMBEDDING_DEADLINE: Duration = Duration::from_secs(3);

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
    #[error("embedding HTTP client could not be initialized")]
    ClientInitializationFailed {
        #[source]
        source: reqwest::Error,
    },
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
    #[error("embedding request did not complete within {deadline:?}")]
    DeadlineExceeded { deadline: Duration },
    #[error("embedding provider returned HTTP {status}")]
    ApiFailure { status: reqwest::StatusCode },
    #[error("embedding provider response is invalid: {reason}")]
    InvalidResponse { reason: &'static str },
}

impl EmbeddingError {
    /// An allowlisted label for terminal diagnostics. Unlike the error text, it never includes
    /// request URLs, credentials or provider response content.
    pub fn diagnostic_category(&self) -> &'static str {
        match self {
            Self::InvalidInput { .. } => "input_invalid",
            Self::ClientInitializationFailed { .. } => "configuration",
            Self::AuthenticationFailed { .. } => "authentication",
            Self::DeadlineExceeded { .. } => "timeout",
            Self::RequestFailed { source } if source.is_timeout() => "timeout",
            Self::RequestFailed { source } if source.is_decode() => "response_invalid",
            Self::RequestFailed { .. } => "request",
            Self::ApiFailure { .. } => "http_status",
            Self::InvalidResponse { .. } => "response_invalid",
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::ApiFailure { status } => Some(status.as_u16()),
            Self::RequestFailed { source } => source.status().map(|status| status.as_u16()),
            Self::InvalidInput { .. }
            | Self::ClientInitializationFailed { .. }
            | Self::AuthenticationFailed { .. }
            | Self::DeadlineExceeded { .. }
            | Self::InvalidResponse { .. } => None,
        }
    }
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

    /// Serves interactive requests: implementations must return, with an error if necessary,
    /// within [`SEARCH_QUERY_EMBEDDING_DEADLINE`] so callers can still answer without it.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_provider_failures_by_variant() {
        for (error, category, status) in [
            (
                EmbeddingError::AuthenticationFailed {
                    source: Box::new(std::io::Error::other("token refresh failed")),
                },
                "authentication",
                None,
            ),
            (
                EmbeddingError::ApiFailure {
                    status: reqwest::StatusCode::TOO_MANY_REQUESTS,
                },
                "http_status",
                Some(429),
            ),
            (
                EmbeddingError::InvalidResponse {
                    reason: "missing embedding values",
                },
                "response_invalid",
                None,
            ),
            (
                EmbeddingError::DeadlineExceeded {
                    deadline: Duration::from_secs(2),
                },
                "timeout",
                None,
            ),
        ] {
            assert_eq!(category, error.diagnostic_category());
            assert_eq!(status, error.http_status());
        }
    }

    #[test]
    fn should_reject_invalid_embedding_vectors() {
        assert!(EmbeddingVector::try_new(vec![1.0; EMBEDDING_DIMENSIONS - 1]).is_err());
        assert!(EmbeddingVector::try_new(vec![0.0; EMBEDDING_DIMENSIONS]).is_err());
        let mut non_finite = vec![1.0; EMBEDDING_DIMENSIONS];
        non_finite[0] = f32::NAN;
        assert!(EmbeddingVector::try_new(non_finite).is_err());
    }
}
