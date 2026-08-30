use listing_source_core::ListingSourceId;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ListingSourceOverview {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    pub crawl_enabled: bool,
    pub llm_calls_count: i64,
    pub domain_count: i64,
    pub last_successful_crawl: Option<time::OffsetDateTime>,
    pub pending_reviews: i64,
    pub product_urls: i64,
    pub blocked_urls: i64,
}
