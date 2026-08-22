use localization::{Language, Localized};
use money::Price;
use product_core::{description::Description, product_image::ProductImage, title::Title};
use product_core::{product_state::ProductState, shops_product_id::ShopsProductId};
use std::collections::BTreeMap;
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
    pub seller_name: Option<String>,
    pub state: ProductState,
    pub url: Url,
    pub images: Vec<ProductImage>,
    pub auction_start: Option<OffsetDateTime>,
    pub auction_end: Option<OffsetDateTime>,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}
