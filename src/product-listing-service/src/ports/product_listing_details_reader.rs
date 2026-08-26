#![allow(dead_code)]

use crate::use_cases::queries::get_product_listing::ProductListingLookup;
use crate::user_state::ProductListingUserState;
use application::personalized::Personalized;
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    ListingSaleObservation, ProductListingAddress, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDetailsReadRequest {
    pub lookup: ProductListingLookup,
    pub language: Language,
    pub user_id: Option<UserId>,
}

/// Factual relational product detail. The use case owns currency presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDetailsReadModel {
    pub product_listing_id: ProductListingId,
    pub product_listing_slug_id: ProductListingSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shop_listing_id: ShopListingId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductListingAddress,
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
    pub auction: ProductListingAuction,
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
