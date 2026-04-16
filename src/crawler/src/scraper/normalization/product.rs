use common::{
    language::domain::Language, localized::Localized, price::domain::Price,
    product_state::domain::ProductState, shops_product_id::ShopsProductId,
};
use product::core::{
    authenticity::Authenticity, condition::Condition, description::Description,
    origin_year::OriginYear, product_image::ProductImage, provenance::Provenance,
    restoration::Restoration, title::Title,
};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedProduct {
    pub shops_product_id: ShopsProductId,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub origin_year: Option<OriginYear>,
    pub authenticity: Authenticity,
    pub condition: Condition,
    pub provenance: Provenance,
    pub restoration: Restoration,
}
