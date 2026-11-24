//! The port for product embeddings computed asynchronously in a provider batch, at a lower price
//! than synchronous requests. The batch embeds each listing exactly as
//! [`EmbeddingGenerator::embed_product`](crate::EmbeddingGenerator::embed_product) does, so
//! vectors from both paths share one index.

use crate::{EmbeddingError, EmbeddingImageUrl, EmbeddingText, EmbeddingVector};

/// One product document of a batch. `key` identifies it in the batch's results.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductEmbeddingRequest {
    pub key: u32,
    pub title: EmbeddingText,
    pub additional_text: Option<EmbeddingText>,
    pub image_url: Option<EmbeddingImageUrl>,
}

/// The provider's handle of a submitted batch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingBatchId(String);

impl EmbeddingBatchId {
    const MAX_CHARS: usize = 512;

    pub fn new(value: impl Into<String>) -> Result<Self, EmbeddingError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > Self::MAX_CHARS
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_/".contains(&byte))
        {
            return Err(EmbeddingError::InvalidInput {
                reason: "embedding batch ID is invalid",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EmbeddingBatchId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug)]
pub enum EmbeddingBatchPoll {
    InProgress,
    Ended(EndedEmbeddingBatch),
}

#[derive(Debug)]
pub struct EndedEmbeddingBatch {
    /// The answered requests by key. A key absent here went unanswered.
    pub results: Vec<(u32, Result<EmbeddingVector, EmbeddingError>)>,
    pub unanswered: UnansweredEmbeddings,
}

/// What the provider's end state says about the requests it did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnansweredEmbeddings {
    /// The batch did not judge them: it was canceled, expired or failed as a whole.
    NotProcessed,
    /// The batch ran every request, so one it did not answer failed on its own.
    Failed,
}

/// Computes product embeddings asynchronously in provider batches.
#[async_trait::async_trait]
pub trait ProductEmbeddingBatches: Send + Sync {
    /// Submits `requests` as one batch. `label` is unique per batch and consists of lowercase
    /// ASCII letters, digits and `-`; the provider may use it to name what it stages.
    async fn submit(
        &self,
        label: &str,
        requests: &[ProductEmbeddingRequest],
    ) -> Result<EmbeddingBatchId, EmbeddingError>;

    async fn poll(&self, batch: &EmbeddingBatchId) -> Result<EmbeddingBatchPoll, EmbeddingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_provider_resource_names_and_reject_other_text() {
        assert!(EmbeddingBatchId::new("projects/123/locations/eu/batchPredictionJobs/456").is_ok());
        for invalid in ["", "job id", "projects/1?x=2", &"a".repeat(513)] {
            assert!(EmbeddingBatchId::new(invalid).is_err(), "{invalid}");
        }
    }
}
