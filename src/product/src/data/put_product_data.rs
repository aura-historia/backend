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
pub struct PutProductData {
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<Option<PriceData>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<Option<PriceData>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<Option<PriceData>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<Url>>,
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
