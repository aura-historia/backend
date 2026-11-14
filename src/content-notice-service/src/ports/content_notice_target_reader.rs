use application::error::BoxError;
use content_notice_core::content_notice_target::{ListingSourceSnapshot, ProductListingSnapshot};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};

/// A listing source as the target of a new notice, with its current snapshot values.
#[derive(Debug, Clone, PartialEq)]
pub struct ListingSourceTarget {
    pub listing_source_id: ListingSourceId,
    pub snapshot: ListingSourceSnapshot,
}

/// A product listing as the target of a new notice. The lifecycle is returned rather than
/// filtered so the submission use case owns the visibility rule.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingTarget {
    pub product_listing_id: ProductListingId,
    pub lifecycle: ListingLifecycle,
    pub snapshot: ProductListingSnapshot,
    pub listing_source: ListingSourceTarget,
}

#[derive(Debug, thiserror::Error)]
pub enum ContentNoticeTargetReadError {
    #[error("content notice target lookup temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid content notice target read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

/// Resolves the target of a new notice. Each lookup is one query.
#[async_trait::async_trait]
pub trait ContentNoticeTargetReader: Send {
    async fn find_listing_source(
        &mut self,
        listing_source_id: ListingSourceId,
    ) -> Result<Option<ListingSourceTarget>, ContentNoticeTargetReadError>;

    async fn find_product_listing(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingTarget>, ContentNoticeTargetReadError>;
}

pub trait ContentNoticeTargetReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ContentNoticeTargetReader + 'tx;
}
