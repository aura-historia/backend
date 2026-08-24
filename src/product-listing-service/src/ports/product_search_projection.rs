use crate::ports::ProductSearchFilterMatchSource;
use application::error::BoxError;
use fxrate_core::FxRateSnapshot;
use product_listing_core::product_id::ProductId;

/// Writes the rebuildable Product search projection. PostgreSQL remains authoritative.
#[async_trait::async_trait]
pub trait ProductSearchProjection: Send + Sync {
    async fn upsert(
        &self,
        source: &ProductSearchFilterMatchSource,
        sale_snapshot: Option<&FxRateSnapshot>,
    ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError>;

    async fn delete(
        &self,
        product_id: ProductId,
        source_version: i64,
    ) -> Result<ProductSearchProjectionWriteOutcome, ProductSearchProjectionWriteError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductSearchProjectionWriteOutcome {
    Applied,
    Stale,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchProjectionWriteError {
    #[error("Product search projection write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("Product search projection delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
}
