use crate::ports::ProductListingSearchFilterMatchSource;
use application::error::BoxError;
use product_listing_core::product_listing_id::ProductListingId;

/// Writes the rebuildable ProductListing search projection. PostgreSQL remains authoritative.
/// Completion means the write was accepted or fenced as stale; search is eventually consistent.
#[async_trait::async_trait]
pub trait ProductListingSearchProjection: Send + Sync {
    async fn upsert(
        &self,
        source: &ProductListingSearchFilterMatchSource,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>;

    /// Replaces the document with a content-free tombstone at `source_version`. An equal
    /// version replaces a document because one source version only changes search eligibility
    /// through a changed projection rule, which reprojection must be able to apply.
    async fn delete(
        &self,
        product_listing_id: ProductListingId,
        source_version: i64,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingSearchProjectionWriteOutcome {
    /// The target accepted the write; search visibility may still lag.
    Applied,
    /// A newer external version already won, or an equal one for an upsert.
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
