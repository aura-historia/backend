use money::Price;
use product_listing_core::product_state::ProductState;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawExpectation {
    pub shop_listing_id: String,
    pub title: String,
    pub description: Vec<String>,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub seller_name: Option<String>,
    pub state: String,
    pub images: Vec<String>,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormalizedExpectation {
    pub shop_listing_id: String,
    pub title: String,
    pub description: Option<String>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub seller_name: Option<String>,
    pub state: ProductState,
    pub url: String,
    pub images: Vec<String>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedExpectationJson {
    pub shop_listing_id: String,
    pub title: String,
    pub description: Option<String>,
    pub price: Option<u64>,
    pub price_currency: Option<String>,
    pub price_estimate_min: Option<u64>,
    pub price_estimate_min_currency: Option<String>,
    pub price_estimate_max: Option<u64>,
    pub price_estimate_max_currency: Option<String>,
    pub seller_name: Option<String>,
    pub state: String,
    pub url: String,
    pub images: Vec<String>,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
}
