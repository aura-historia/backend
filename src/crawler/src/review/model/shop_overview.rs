use serde::Serialize;
use shop_core::shop_id::ShopId;

#[derive(Debug, Serialize)]
pub struct ShopOverview {
    pub shop_id: ShopId,
    pub shop_name: Option<String>,
    pub llm_calls_count: i64,
    pub url_pattern: Option<String>,
    pub pending_reviews: i64,
    pub product_urls: i64,
    pub blocked_urls: i64,
}
