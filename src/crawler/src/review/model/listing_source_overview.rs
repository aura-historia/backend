use crate::CrawlerReviewId;
use listing_source_core::ListingSourceId;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ListingSourceOverview {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    pub crawl_enabled: bool,
    pub llm_calls_count: i64,
    /// Where the source's crawl starts, derived from its presentation URL; `None` while the scope
    /// gives it no crawl root (`docs/crawler/README.md#listingsource-scope`).
    pub crawl_root_url: Option<String>,
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
    /// scraped until it is decided: those whose scrape recorded the wait, and every product URL
    /// of a ListingSource whose first product-schema review is pending.
    pub blocked_urls: i64,
    /// Since when the ListingSource has been blocked: the creation of the oldest pending review
    /// a blocked URL waits on. `None` while `blocked_urls` is 0.
    #[serde(with = "time::serde::rfc3339::option")]
    pub blocked_since: Option<time::OffsetDateTime>,
    /// The pending product-schema review the blocked URLs wait on: the oldest one, the review
    /// whose creation is `blocked_since`. `None` while `blocked_urls` is 0.
    pub blocking_review_id: Option<CrawlerReviewId>,
    /// Product URLs whose removal the mass-removal guard held back, which keep their listing.
    pub held_removal_urls: i64,
    /// The earliest time a currently held URL was last held: the guard records a held URL as
    /// scraped without a capture, so this is the earliest `last_scraped` among them. A scrape that
    /// holds a URL again moves it, so a hold that repeats is dated by its latest pass, not by its
    /// start. `None` while `held_removal_urls` is 0.
    #[serde(with = "time::serde::rfc3339::option")]
    pub held_since: Option<time::OffsetDateTime>,
    /// The last error kind of the product URLs that carry one, with the number of URLs per kind,
    /// the most frequent first.
    pub url_error_kinds: Vec<UrlErrorKindCount>,
    /// Product pages whose last scrape recorded `MissingRequiredValue`, listed by
    /// `GET /api/listing-sources/{id}/missing-value-pages`.
    pub missing_value_urls: i64,
    /// How the spider and the scraper last left the source, so an operator sees whether it waits
    /// for a review or for an operator, or keeps failing to scrape, without reading the logs.
    #[serde(flatten)]
    pub crawl_state: CrawlStateOverview,
}

/// How many product URLs of a ListingSource last ended with one error kind.
#[derive(Debug, Serialize)]
pub struct UrlErrorKindCount {
    pub kind: String,
    pub count: i64,
}

/// How the spider and the scraper last left one ListingSource.
#[derive(Debug, Default, Serialize)]
pub struct CrawlStateOverview {
    pub last_crawl_outcome: Option<String>,
    pub last_crawl_error_kind: Option<String>,
    pub crawl_failure_count: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_crawl_at: Option<time::OffsetDateTime>,
    /// The source's consecutive scrape failures of one kind, as its scraper circuit records them;
    /// 0 once a fetch of it succeeds again.
    pub scrape_failure_streak: i32,
    /// The kind of those failures, such as `TIMEOUT` or `HTTP_503`; `None` without a streak.
    pub last_scrape_error_kind: Option<String>,
    /// When the current same-kind scrape failure streak began; `None` while
    /// `scrape_failure_streak` is 0.
    #[serde(with = "time::serde::rfc3339::option")]
    pub scrape_failing_since: Option<time::OffsetDateTime>,
    /// The end of the scraper circuit's cooldown, after which one probe URL is fetched: the
    /// stored `next_scrape_at`. `None` while the circuit is closed or the streak is recorded
    /// evidence without a cooldown.
    #[serde(with = "time::serde::rfc3339::option")]
    pub scrape_paused_until: Option<time::OffsetDateTime>,
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
            crawl_root_url: Some("https://example.com/".to_owned()),
            last_successful_crawl: last_successful,
            last_successful_scrape: last_successful,
            pending_reviews: 0,
            product_urls: 0,
            blocked_urls: 0,
            blocked_since: None,
            blocking_review_id: None,
            held_removal_urls: 0,
            held_since: None,
            url_error_kinds: Vec::new(),
            missing_value_urls: 0,
            crawl_state: CrawlStateOverview {
                last_crawl_outcome: Some("NEEDS_OPERATOR".to_owned()),
                last_crawl_error_kind: Some("NoUrlPattern".to_owned()),
                crawl_failure_count: 5,
                next_crawl_at: None,
                scrape_failure_streak: 3,
                last_scrape_error_kind: Some("HTTP_503".to_owned()),
                scrape_failing_since: None,
                scrape_paused_until: None,
            },
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
    fn crawl_state_serializes_root_outcome_and_failure_kind_on_the_source() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert_eq!(json["crawl_root_url"], "https://example.com/");
        assert_eq!(json["last_crawl_outcome"], "NEEDS_OPERATOR");
        assert_eq!(json["last_crawl_error_kind"], "NoUrlPattern");
        assert_eq!(json["crawl_failure_count"], 5);
        assert!(json["next_crawl_at"].is_null());
    }

    #[test]
    fn scrape_state_serializes_failure_streak_and_kind_on_the_source() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert_eq!(json["scrape_failure_streak"], 3);
        assert_eq!(json["last_scrape_error_kind"], "HTTP_503");
    }

    #[test]
    fn blocking_review_held_since_and_scrape_times_serialize() {
        let mut source = overview(None);
        let review_id = CrawlerReviewId::new();
        source.blocking_review_id = Some(review_id);
        source.held_since = Some(datetime!(2026-10-31 08:00:00 UTC));
        source.crawl_state.scrape_failing_since = Some(datetime!(2026-11-01 10:00:00 UTC));
        source.crawl_state.scrape_paused_until = Some(datetime!(2026-11-01 12:00:00 UTC));

        let json = serde_json::to_value(source).unwrap();

        assert_eq!(json["blocking_review_id"], review_id.to_string());
        assert_eq!(json["held_since"], "2026-10-31T08:00:00Z");
        assert_eq!(json["scrape_failing_since"], "2026-11-01T10:00:00Z");
        assert_eq!(json["scrape_paused_until"], "2026-11-01T12:00:00Z");
    }

    #[test]
    fn absent_blocking_review_held_since_and_scrape_times_serialize_as_null() {
        let json = serde_json::to_value(overview(None)).unwrap();

        assert!(json["blocking_review_id"].is_null());
        assert!(json["held_since"].is_null());
        assert!(json["scrape_failing_since"].is_null());
        assert!(json["scrape_paused_until"].is_null());
    }
}
