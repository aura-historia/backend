#![allow(dead_code)]

use crate::use_cases::queries::get_product_listing::ProductListingLookup;
use crate::{ports::ListingSourceSummary, user_state::ProductListingUserState};
use application::personalized::Personalized;
use auction_core::{AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus, AuctionSchedule};
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{ListingSaleObservation, ProductListingPricing};
use product_listing_core::product_listing_auction::{CataloguePosition, LotNumber};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::source_listing_id::SourceListingId;

use product_listing_core::title::Title;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDetailsReadRequest {
    pub lookup: ProductListingLookup,
    pub language: Language,
    pub user_id: Option<UserId>,
}

/// Safe current parent Auction presentation for ProductListing full-detail reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingAuctionSummary {
    pub auction_id: AuctionId,
    pub name: Option<Localized<Language, AuctionName>>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub schedule: AuctionSchedule,
}

/// Listing-owned lot facts shown beside the current parent Auction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingLot {
    pub lot_number: Option<LotNumber>,
    pub catalogue_position: Option<CataloguePosition>,
    pub bidding_opens: Option<OffsetDateTime>,
    pub scheduled_closes: Option<OffsetDateTime>,
    pub reported_closed_at: Option<OffsetDateTime>,
}

/// Factual relational product detail. The use case owns currency presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDetailsReadModel {
    pub product_listing_id: ProductListingId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub event_id: EventId,
    pub source: ListingSourceSummary,
    pub source_listing_id: SourceListingId,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub sale_observation: Option<ListingSaleObservation>,
    pub availability: Option<ListingAvailability>,
    pub lifecycle: ListingLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub content_policy: Option<ContentPolicyDecision>,
    pub auction: Option<ProductListingAuctionSummary>,
    pub lot: Option<ProductListingLot>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type PersonalizedProductListingDetailsReadModel =
    Personalized<ProductListingDetailsReadModel, ProductListingUserState>;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingDetailsReadError {
    #[error("product details query failed")]
    ProductListingDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductListingDetailsReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductListingDetailsReader: Send {
    async fn find_details(
        &mut self,
        request: &ProductListingDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductListingDetailsReadModel>, ProductListingDetailsReadError>;
}

pub trait ProductListingDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingDetailsReader + 'tx;
}
