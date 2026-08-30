use localization::{Language, Localized};
use product_listing_core::product_listing_slug_id::{
    InvalidProductListingSlugId, ProductListingSlugId,
};
use product_listing_core::title::Title;

pub const MAX_PRODUCT_LISTING_TITLE_SLUG_INSERT_ATTEMPTS: usize = 5;

/// Selects one random public locator candidate for an application creation attempt.
pub fn next_product_listing_title_slug(
    title: Option<&Localized<Language, Title>>,
) -> Result<ProductListingSlugId, InvalidProductListingSlugId> {
    next_product_listing_title_slug_from_text(title.map_or("", |value| value.payload.as_ref()))
}

pub fn next_product_listing_title_slug_from_text(
    title: &str,
) -> Result<ProductListingSlugId, InvalidProductListingSlugId> {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    ProductListingSlugId::from_title_and_suffix(title, &suffix[..6])
}
