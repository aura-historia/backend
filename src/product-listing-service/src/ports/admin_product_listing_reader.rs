use super::{ListingSourceSummary, ProductListingStorageVersion};
use application::{
    error::BoxError,
    pagination::{Cursor, CursoredResult},
};
use auction_core::AuctionId;
use domain_primitives::sort::SortOrder;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use time::OffsetDateTime;
use url::Url;

/// Shared admin/partner management models. Readers do not authorize callers.
/// Authoritative original facts. Image URLs are privileged inspection data.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminProductListingDetails {
    pub summary: AdminProductListingSummary,
    pub description: Option<Localized<Language, Description>>,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: Option<ProductListingAuction>,
    pub sale_observation: Option<ListingSaleObservation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminProductListingSummary {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub source: ListingSourceSummary,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub lifecycle: ListingLifecycle,
    pub auction_id: Option<AuctionId>,
    pub version: ProductListingStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdminProductListingSort {
    Created,
    #[default]
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProductListingSearchScope {
    pub query: Option<String>,
    pub listing_source_id: Option<ListingSourceId>,
    pub source_listing_id: Option<SourceListingId>,
    pub lifecycle: Option<ListingLifecycle>,
    pub availability: Option<ListingAvailability>,
    pub auction_id: Option<AuctionId>,
    pub sort: AdminProductListingSort,
    pub order: SortOrder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProductListingSearchCursor {
    pub product_listing_id: ProductListingId,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub scope: AdminProductListingSearchScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminProductListingSearchRequest {
    pub scope: AdminProductListingSearchScope,
    pub cursor: Cursor<AdminProductListingSearchCursor>,
}

pub type AdminProductListingSearchResult =
    CursoredResult<AdminProductListingSummary, AdminProductListingSearchCursor>;

#[derive(Debug, thiserror::Error)]
pub enum AdminProductListingReadError {
    #[error("temporary admin product listing read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted admin product listing state")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("admin product listing query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AdminProductListingDetailsReader: Send + Sync {
    async fn find_by_id(
        &self,
        id: ProductListingId,
    ) -> Result<Option<AdminProductListingDetails>, AdminProductListingReadError>;
    async fn find_by_id_in_source(
        &self,
        source: ListingSourceId,
        id: ProductListingId,
    ) -> Result<Option<AdminProductListingDetails>, AdminProductListingReadError>;
}

#[async_trait::async_trait]
pub trait AdminProductListingSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &AdminProductListingSearchRequest,
    ) -> Result<AdminProductListingSearchResult, AdminProductListingReadError>;
}
