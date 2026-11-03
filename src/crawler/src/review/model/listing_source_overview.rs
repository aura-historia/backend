use crate::CrawlerDomainId;
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
    /// When a product page of the ListingSource was last scraped, the latest `last_scraped` of its
    /// product URLs: a page the scraper evaluated, also one that left a required value empty,
    /// which a failed fetch, a wait on a pending review or a removal the mass-removal guard held
    /// does not move. `None` before its first scraped page.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_successful_scrape: Option<time::OffsetDateTime>,
    pub pending_reviews: i64,
    pub product_urls: i64,
    /// Product URLs waiting on a product-schema review that is still pending, and so not
    /// scraped until it is decided.
    pub blocked_urls: i64,
    /// Since when the ListingSource has been blocked: the creation of the oldest pending review
    /// a blocked URL waits on. `None` while `blocked_urls` is 0.
    #[serde(with = "time::serde::rfc3339::option")]
    pub blocked_since: Option<time::OffsetDateTime>,
    /// Product URLs whose removal the mass-removal guard held back, which keep their listing.
    pub held_removal_urls: i64,
    /// The last error kind of the product URLs that carry one, with the number of URLs per kind,
    /// the most frequent first.
    pub url_error_kinds: Vec<UrlErrorKindCount>,
    /// Product pages whose last scrape recorded `MissingRequiredValue`, listed by
    /// `GET /api/listing-sources/{id}/missing-value-pages`.
    pub missing_value_urls: i64,
    /// Crawl and scrape state of each configured domain, so an operator sees which domain
    /// waits for a review or for an operator, or keeps failing to scrape, without reading the logs.
    pub domains: Vec<DomainCrawlOverview>,
}

/// How many product URLs of a ListingSource last ended with one error kind.
#[derive(Debug, Serialize)]
pub struct UrlErrorKindCount {
    pub kind: String,
    pub count: i64,
}

/// How the spider and the scraper last left one configured domain.
#[derive(Debug, Serialize)]
pub struct DomainCrawlOverview {
    pub domain_id: CrawlerDomainId,
    pub domain: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_crawled: Option<time::OffsetDateTime>,
    pub last_crawl_outcome: Option<String>,
    pub last_crawl_error_kind: Option<String>,
    pub crawl_failure_count: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_crawl_at: Option<time::OffsetDateTime>,
    /// The domain's consecutive scrape failures of one kind, as its scraper circuit records them;
    /// 0 once a fetch of it succeeds again.
    pub scrape_failure_streak: i32,
    /// The kind of those failures, such as `TIMEOUT` or `HTTP_503`; `None` without a streak.
    pub last_scrape_error_kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn overview(last_successful: Option<time::OffsetDateTime>) -> ListingSourceOverview {
        ListingSourceOverview {
            listing_source_id: ListingSourceId::try_from(uuid::Uuid::now_v7())
                .expect("v7 uuid is a valid listing source id"),
            listing_source_name: None,
            crawl_enabled: true,
            llm_calls_count: 0,
            domain_count: 1,
            last_successful_crawl: last_successful,
            last_successful_scrape: last_successful,
            pending_reviews: 0,
            product_urls: 0,
            blocked_urls: 0,
            blocked_since: None,
            held_removal_urls: 0,
            url_error_kinds: Vec::new(),
            missing_value_urls: 0,
            domains: vec![DomainCrawlOverview {
                domain_id: CrawlerDomainId::new(),
                domain: "example.com".to_owned(),
                last_crawled: None,
                last_crawl_outcome: Some("NEEDS_OPERATOR".to_owned()),
                last_crawl_error_kind: Some("NoUrlPattern".to_owned()),
                crawl_failure_count: 5,
                next_crawl_at: None,
                scrape_failure_streak: 3,
                last_scrape_error_kind: Some("HTTP_503".to_owned()),
            }],
        }
    }

    #[test]
    fn last_successful_crawl_and_scrape_serialize_as_rfc3339_strings() {
        let json =
            serde_json::to_value(overview(Some(datetime!(2026-10-15 13:29:45 UTC)))).unwrap();

        assert_eq!(json["last_successful_crawl"], "2026-10-15T13:29:45Z");
        assert_eq!(json["last_successful_scrape"], "2026-10-15T13:29:45Z");
    }

    #[test]
    fn missing_last_successful_crawl_and_scrape_serialize_as_null() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert!(json["last_successful_crawl"].is_null());
        assert!(json["last_successful_scrape"].is_null());
    }

    #[test]
    fn blocked_held_and_error_kind_fields_serialize() {
        let mut source = overview(None);
        source.blocked_urls = 4;
        source.blocked_since = Some(datetime!(2026-10-15 13:29:45 UTC));
        source.held_removal_urls = 2;
        source.url_error_kinds = vec![UrlErrorKindCount {
            kind: "MassRemovalHeld".to_owned(),
            count: 2,
        }];

        let json = serde_json::to_value(source).unwrap();

        assert_eq!(json["blocked_since"], "2026-10-15T13:29:45Z");
        assert_eq!(json["held_removal_urls"], 2);
        assert_eq!(json["url_error_kinds"][0]["kind"], "MassRemovalHeld");
        assert_eq!(json["url_error_kinds"][0]["count"], 2);
    }

    #[test]
    fn domain_crawl_state_serializes_outcome_and_failure_kind() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert_eq!(json["domains"][0]["domain"], "example.com");
        assert_eq!(json["domains"][0]["last_crawl_outcome"], "NEEDS_OPERATOR");
        assert_eq!(json["domains"][0]["last_crawl_error_kind"], "NoUrlPattern");
        assert_eq!(json["domains"][0]["crawl_failure_count"], 5);
        assert!(json["domains"][0]["next_crawl_at"].is_null());
    }

    #[test]
    fn domain_scrape_state_serializes_failure_streak_and_kind() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert_eq!(json["domains"][0]["scrape_failure_streak"], 3);
        assert_eq!(json["domains"][0]["last_scrape_error_kind"], "HTTP_503");
    }
}
