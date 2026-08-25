use crate::ports::ProductListingSearchFilterMatchSource;
use application::error::BoxError;
use fxrate_core::FxRateSnapshot;
use product_listing_core::product_listing_id::ProductListingId;

/// Writes the rebuildable ProductListing search projection. PostgreSQL remains authoritative.
#[async_trait::async_trait]
pub trait ProductListingSearchProjection: Send + Sync {
    async fn upsert(
        &self,
        source: &ProductListingSearchFilterMatchSource,
        sale_snapshot: Option<&FxRateSnapshot>,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>;

    async fn delete(
        &self,
        product_id: ProductListingId,
        source_version: i64,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingSearchProjectionWriteOutcome {
    Applied,
    Stale,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchProjectionWriteError {
    #[error("ProductListing search projection write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("ProductListing search projection delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
}
