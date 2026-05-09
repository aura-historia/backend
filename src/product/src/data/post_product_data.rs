use crate::data::product_state_data::ProductStateData;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shops_product_id::ShopsProductId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostProductData {
    pub shops_product_id: ShopsProductId,
    pub title: LocalizedTextData,
    pub description: LocalizedTextData,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
    pub state: ProductStateData,
    pub url: Url,
    pub images: Vec<Url>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_end: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddressData>,
}
