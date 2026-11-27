//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use listing_source_core::ListingSourceId;
use serde::Serialize;
use time::OffsetDateTime;

/// What the operator sees first: what the crawler does right now, how well it keeps up and what
/// needs a human. The crawler keeps each source's and URL's current state, not a history, so the
/// rates count the sources and URLs whose latest work falls in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsoleOverview {
    pub sources: SourceCounts,
    pub freshness: Freshness,
    pub throughput: Throughput,
    pub crawls: CrawlFreshness,
    pub pages: PageFreshness,
    pub failures: FailureKinds,
    pub reviews: ReviewCounts,
    pub review_flow: ReviewFlow,
    /// Whether review agents work the reviews, so a review can wait for a human.
    pub agent_mode: bool,
}

/// How the ListingSources stand. The six conditions overlap, and `needs_attention` counts a
/// source once whichever of them it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceCounts {
    /// ListingSources this process is crawling right now.
    pub crawling_now: i64,
    /// ListingSources this process is scraping right now.
    pub scraping_now: i64,
    pub needs_attention: i64,
    /// Sources whose last crawl failed and that the schedule retries.
    pub crawl_failing: i64,
    pub crawl_needs_operator: i64,
    pub blocked_by_review: i64,
    pub failing_to_scrape: i64,
    pub held_removals: i64,
    /// Sources with product URLs in an identity conflict.
    pub identity_conflicts: i64,
    /// Sources a human or an agent labelled, whose crawling is off; not a condition that needs
    /// attention.
    pub labelled: i64,
    /// Sources the review agents parked until they try them again; informational.
    pub parked: i64,
    /// Crawl-enabled sources neither the scraper nor the spider worked for
    /// [`Freshness::stale_after_days`]; informational, as the `stale` list counts them.
    pub stale: i64,
    /// Crawl-enabled sources whose shop's rate limit holds their next crawl back right now.
    pub rate_limited: i64,
    /// Crawl-enabled sources whose scraper circuit pauses their scrapes right now.
    pub scrape_paused: i64,
}

/// How long ago the crawler last worked the crawl-enabled ListingSources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Freshness {
    /// After how many days without work a crawl-enabled source counts as stale.
    pub stale_after_days: i64,
    /// The time since each source's last scraped page; `None` while no source has one.
    pub since_last_scrape: Option<AgeDistribution>,
    /// The time since each source's last spider run of any outcome; `None` while no source has
    /// one.
    pub since_last_crawl: Option<AgeDistribution>,
    /// The first [`LONGEST_IDLE_SOURCES`] stale sources, the longest idle first, as the `stale`
    /// list orders them by `idle_since`.
    pub longest_idle: Vec<IdleSource>,
}

/// How many stale sources [`Freshness::longest_idle`] names.
pub const LONGEST_IDLE_SOURCES: usize = 5;

/// A stale ListingSource and when the crawler last worked it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdleSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    /// The later of its last scraped page and its last spider run; `None` when it was never
    /// worked.
    #[serde(with = "time::serde::rfc3339::option")]
    pub idle_since: Option<OffsetDateTime>,
}

/// A distribution of ages, in seconds, over the sources that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgeDistribution {
    pub sources: i64,
    pub median_secs: i64,
    pub mean_secs: i64,
    pub p90_secs: i64,
    pub max_secs: i64,
}

/// The work done in the last hour and day. A URL or source counts once, by its latest scrape or
/// run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Throughput {
    /// Product pages whose last scrape evaluated the page within the window.
    pub pages_scraped_1h: i64,
    pub pages_scraped_24h: i64,
    /// Product URLs the spider found within the last day.
    pub product_urls_found_24h: i64,
    /// Sources whose last spider run ended within the window.
    pub crawls_finished_1h: i64,
    pub crawls_finished_24h: i64,
    /// The sources by the clock hour (UTC) their last spider run ended in, the oldest hour
    /// first and the running hour last. The spider runs a source once a day, so this is close
    /// to the runs of each hour; a source that ran again since counts in its later hour only.
    pub crawl_runs_by_hour: [HourlyCrawlRuns; CRAWL_RUN_HOURS],
}

/// How many clock hours [`Throughput::crawl_runs_by_hour`] covers, the running one included.
pub const CRAWL_RUN_HOURS: usize = 24;

/// One clock hour of [`Throughput::crawl_runs_by_hour`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HourlyCrawlRuns {
    /// When the hour began.
    #[serde(with = "time::serde::rfc3339")]
    pub hour: OffsetDateTime,
    /// Sources whose last spider run ended in the hour.
    pub sources: i64,
}

/// The crawl-enabled ListingSources by the end of their last spider run of any outcome. A source
/// is due for its next crawl a day after its last, so the four parts say how well the spider
/// keeps up; they sum to `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CrawlFreshness {
    pub total: i64,
    pub crawled_within_a_day: i64,
    /// Last crawled over a day ago, up to [`Freshness::stale_after_days`] days.
    pub due: i64,
    /// Last crawled over [`Freshness::stale_after_days`] days ago.
    pub overdue: i64,
    pub never_crawled: i64,
}

/// The product pages of crawl-enabled ListingSources by their last scrape. A page is due for
/// its next scrape a day after its last, so the four parts say how well the scraper keeps up;
/// they sum to `total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PageFreshness {
    pub total: i64,
    pub scraped_within_a_day: i64,
    /// Last scraped over a day ago, up to [`Freshness::stale_after_days`] days: the daily
    /// scrape is behind on these.
    pub due: i64,
    /// Last scraped over [`Freshness::stale_after_days`] days ago.
    pub overdue: i64,
    /// Never scraped, including the pages that wait for a source's first schema.
    pub never_scraped: i64,
}

/// What the crawl-enabled ListingSources fail with, the most frequent first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureKinds {
    /// The kinds of the scrape failure streaks; they sum to `failing_to_scrape`.
    pub scrape: Vec<FailureKindCount>,
    /// The kinds of the failed last crawls; they sum to `crawl_failing`.
    pub crawl: Vec<FailureKindCount>,
}

/// How many sources fail with one error kind; `None` for a failure recorded without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureKindCount {
    pub error_kind: Option<String>,
    pub sources: i64,
}

/// How the review queue moved: the reviews opened and decided recently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReviewFlow {
    /// The last [`REVIEW_FLOW_DAYS`] 24-hour windows, the oldest first; the last one ends now.
    pub daily: [DailyReviewFlow; REVIEW_FLOW_DAYS],
    /// The median time from opening to approval or rejection of the reviews decided in the last
    /// 7 days; `None` while none was.
    pub median_decision_secs_7d: Option<i64>,
}

/// How many 24-hour windows [`ReviewFlow::daily`] covers.
pub const REVIEW_FLOW_DAYS: usize = 7;

/// The reviews opened, and those approved or rejected, within one 24-hour window. A superseded
/// review was not decided on its own and counts only as opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct DailyReviewFlow {
    pub opened: i64,
    pub approved: i64,
    pub rejected: i64,
}

/// The pending reviews of all ListingSources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReviewCounts {
    pub pending_total: i64,
    pub pending_by_artifact: PendingByArtifact,
    /// Of `pending_total`, those of ListingSources with crawling disabled, which hold back no
    /// crawl or scrape.
    pub pending_disabled_sources: i64,
    /// Of `pending_total`, those of crawl-enabled ListingSources that wait for a human; none
    /// outside `agent` authoring mode, and normally none in it, since the agents park what they
    /// cannot finish. A review a human opened to edit the live configuration is the humans' own
    /// work and does not count.
    pub waiting_for_human: i64,
    /// Of `pending_total`, those of ListingSources the agents parked, which wait for their retry.
    pub parked: i64,
    /// Of `pending_total`, the product-schema reviews the crawler handed to a human because their
    /// source's URL pattern likely takes non-product pages, in any authoring mode.
    pub held_for_non_product_loop: i64,
    /// When the oldest pending review of a crawl-enabled ListingSource opened; `None` while none
    /// is pending.
    #[serde(with = "time::serde::rfc3339::option")]
    pub oldest_pending: Option<OffsetDateTime>,
    /// The median age of those reviews; `None` while none is pending.
    pub median_pending_age_secs: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PendingByArtifact {
    #[serde(rename = "PRODUCT_SCHEMA")]
    pub product_schema: i64,
    #[serde(rename = "URL_PATTERN")]
    pub url_pattern: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_serialize_the_counts_as_the_contract_names_them() {
        let first_hour = datetime!(2026-11-26 13:00 UTC);
        let mut crawl_runs_by_hour = [HourlyCrawlRuns {
            hour: first_hour,
            sources: 0,
        }; CRAWL_RUN_HOURS];
        for (hours, runs) in (0_i64..).zip(crawl_runs_by_hour.iter_mut()) {
            runs.hour = first_hour + time::Duration::hours(hours);
        }
        crawl_runs_by_hour[23].sources = 2;
        let mut daily = [DailyReviewFlow::default(); REVIEW_FLOW_DAYS];
        daily[6] = DailyReviewFlow {
            opened: 14,
            approved: 10,
            rejected: 3,
        };
        let listing_source_id = ListingSourceId::try_from(uuid::Uuid::now_v7())
            .unwrap_or_else(|error| panic!("{error}"));
        let overview = ConsoleOverview {
            sources: SourceCounts {
                crawling_now: 4,
                scraping_now: 11,
                needs_attention: 23,
                crawl_failing: 2,
                crawl_needs_operator: 6,
                blocked_by_review: 9,
                failing_to_scrape: 7,
                held_removals: 1,
                identity_conflicts: 5,
                labelled: 3,
                parked: 8,
                stale: 12,
                rate_limited: 1,
                scrape_paused: 2,
            },
            freshness: Freshness {
                stale_after_days: 3,
                since_last_scrape: Some(AgeDistribution {
                    sources: 40,
                    median_secs: 3_600,
                    mean_secs: 7_200,
                    p90_secs: 86_400,
                    max_secs: 604_800,
                }),
                since_last_crawl: None,
                longest_idle: vec![IdleSource {
                    listing_source_id,
                    listing_source_name: Some("Idle shop".to_owned()),
                    idle_since: None,
                }],
            },
            throughput: Throughput {
                pages_scraped_1h: 120,
                pages_scraped_24h: 2_400,
                product_urls_found_24h: 35,
                crawls_finished_1h: 2,
                crawls_finished_24h: 30,
                crawl_runs_by_hour,
            },
            crawls: CrawlFreshness {
                total: 40,
                crawled_within_a_day: 30,
                due: 6,
                overdue: 3,
                never_crawled: 1,
            },
            pages: PageFreshness {
                total: 3_000,
                scraped_within_a_day: 2_400,
                due: 440,
                overdue: 100,
                never_scraped: 60,
            },
            failures: FailureKinds {
                scrape: vec![FailureKindCount {
                    error_kind: Some("TIMEOUT".to_owned()),
                    sources: 7,
                }],
                crawl: vec![FailureKindCount {
                    error_kind: None,
                    sources: 2,
                }],
            },
            reviews: ReviewCounts {
                pending_total: 312,
                pending_by_artifact: PendingByArtifact {
                    product_schema: 241,
                    url_pattern: 71,
                },
                pending_disabled_sources: 4,
                waiting_for_human: 9,
                parked: 6,
                held_for_non_product_loop: 2,
                oldest_pending: Some(datetime!(2026-11-20 08:00 UTC)),
                median_pending_age_secs: None,
            },
            review_flow: ReviewFlow {
                daily,
                median_decision_secs_7d: Some(5_400),
            },
            agent_mode: true,
        };

        let mut serialized =
            serde_json::to_value(overview).unwrap_or_else(|error| panic!("{error}"));
        let hours = serialized["throughput"]["crawl_runs_by_hour"].take();
        let days = serialized["review_flow"]["daily"].take();

        assert_eq!(
            serialized,
            json!({
                "sources": {
                    "crawling_now": 4, "scraping_now": 11, "needs_attention": 23,
                    "crawl_failing": 2, "crawl_needs_operator": 6, "blocked_by_review": 9,
                    "failing_to_scrape": 7, "held_removals": 1, "identity_conflicts": 5,
                    "labelled": 3, "parked": 8, "stale": 12, "rate_limited": 1,
                    "scrape_paused": 2
                },
                "freshness": {
                    "stale_after_days": 3,
                    "since_last_scrape": {
                        "sources": 40, "median_secs": 3600, "mean_secs": 7200,
                        "p90_secs": 86400, "max_secs": 604800
                    },
                    "since_last_crawl": null,
                    "longest_idle": [{
                        "listing_source_id": listing_source_id,
                        "listing_source_name": "Idle shop",
                        "idle_since": null
                    }]
                },
                "throughput": {
                    "pages_scraped_1h": 120, "pages_scraped_24h": 2400,
                    "product_urls_found_24h": 35, "crawls_finished_1h": 2,
                    "crawls_finished_24h": 30, "crawl_runs_by_hour": null
                },
                "crawls": {
                    "total": 40, "crawled_within_a_day": 30, "due": 6, "overdue": 3,
                    "never_crawled": 1
                },
                "pages": {
                    "total": 3000, "scraped_within_a_day": 2400, "due": 440, "overdue": 100,
                    "never_scraped": 60
                },
                "failures": {
                    "scrape": [{ "error_kind": "TIMEOUT", "sources": 7 }],
                    "crawl": [{ "error_kind": null, "sources": 2 }]
                },
                "reviews": {
                    "pending_total": 312,
                    "pending_by_artifact": { "PRODUCT_SCHEMA": 241, "URL_PATTERN": 71 },
                    "pending_disabled_sources": 4,
                    "waiting_for_human": 9,
                    "parked": 6,
                    "held_for_non_product_loop": 2,
                    "oldest_pending": "2026-11-20T08:00:00Z",
                    "median_pending_age_secs": null
                },
                "review_flow": { "daily": null, "median_decision_secs_7d": 5400 },
                "agent_mode": true
            })
        );
        assert_eq!(hours.as_array().map(Vec::len), Some(CRAWL_RUN_HOURS));
        assert_eq!(
            (&hours[0], &hours[23]),
            (
                &json!({ "hour": "2026-11-26T13:00:00Z", "sources": 0 }),
                &json!({ "hour": "2026-11-27T12:00:00Z", "sources": 2 })
            )
        );
        assert_eq!(days.as_array().map(Vec::len), Some(REVIEW_FLOW_DAYS));
        assert_eq!(
            days[6],
            json!({ "opened": 14, "approved": 10, "rejected": 3 })
        );
    }
}
