use crate::data::product_state_data::ProductStateData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchProductData {
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<IndexSet<Url>>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub auction_end: Option<OffsetDateTime>,
}
