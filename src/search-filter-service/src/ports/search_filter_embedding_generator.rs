use common::error::boxed::BoxError;
use search_filter_core::ProductSearch;

#[derive(Debug, thiserror::Error)]
pub enum SearchFilterEmbeddingGenerationError {
    #[error("search filter embedding generation failed")]
    GenerationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait SearchFilterEmbeddingGenerator: Send + Sync {
    async fn generate(
        &self,
        search: &ProductSearch,
    ) -> Result<Option<Vec<f32>>, SearchFilterEmbeddingGenerationError>;
}
