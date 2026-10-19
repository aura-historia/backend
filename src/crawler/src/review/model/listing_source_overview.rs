use listing_source_core::ListingSourceId;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ListingSourceOverview {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    pub crawl_enabled: bool,
    pub llm_calls_count: i64,
    pub domain_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_successful_crawl: Option<time::OffsetDateTime>,
    pub pending_reviews: i64,
    pub product_urls: i64,
    pub blocked_urls: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn overview(last_successful_crawl: Option<time::OffsetDateTime>) -> ListingSourceOverview {
        ListingSourceOverview {
            listing_source_id: ListingSourceId::try_from(uuid::Uuid::now_v7())
                .expect("v7 uuid is a valid listing source id"),
            listing_source_name: None,
            crawl_enabled: true,
            llm_calls_count: 0,
            domain_count: 1,
            last_successful_crawl,
            pending_reviews: 0,
            product_urls: 0,
            blocked_urls: 0,
        }
    }

    #[test]
    fn last_successful_crawl_serializes_as_rfc3339_string() {
        let json =
            serde_json::to_value(overview(Some(datetime!(2026-10-15 13:29:45 UTC)))).unwrap();

        assert_eq!(json["last_successful_crawl"], "2026-10-15T13:29:45Z");
    }

    #[test]
    fn missing_last_successful_crawl_serializes_as_null() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert!(json["last_successful_crawl"].is_null());
    }
}
