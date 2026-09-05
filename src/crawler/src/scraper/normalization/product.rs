use localization::{Language, Localized};
use money::Price;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::{
    description::Description, product_listing_image::ProductListingImage, title::Title,
};
use product_listing_normalization::ListingAvailabilityQuickCheck;
use std::collections::BTreeMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedProduct {
    pub source_listing_id: SourceListingId,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    /// Generic boundary outcome. Unsupported values preserve canonical availability.
    pub availability: ListingAvailabilityQuickCheck,
    pub url: Url,
    pub images: Vec<ProductListingImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}
