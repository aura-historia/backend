use crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage, title::Title,
};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedProduct {
    pub shop_listing_id: ShopListingId,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub seller_name: Option<String>,
    /// Boundary decision. `Ignore` must not mutate current aggregate availability.
    pub availability: ListingAvailabilityMapping,
    pub url: Url,
    pub images: Vec<ProductListingImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}
