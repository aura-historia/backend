//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

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
    pub backlog: Backlog,
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
}

/// How long ago the crawler last worked the crawl-enabled ListingSources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Freshness {
    /// After how many days without work a crawl-enabled source counts as stale.
    pub stale_after_days: i64,
    /// The time since each source's last scraped page; `None` while no source has one.
    pub since_last_scrape: Option<AgeDistribution>,
    /// The time since each source's last spider run of any outcome; `None` while no source has
    /// one.
    pub since_last_crawl: Option<AgeDistribution>,
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
}

/// The work of crawl-enabled ListingSources that waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Backlog {
    /// Product pages last scraped over a day ago, which the daily scrape is behind on.
    pub pages_not_scraped_for_a_day: i64,
    /// Product pages never scraped, including those that wait for a source's first schema.
    pub pages_never_scraped: i64,
    /// Sources whose shop's rate limit holds their next crawl back right now.
    pub rate_limited_sources: i64,
    /// Sources whose scraper circuit pauses their scrapes right now.
    pub scrape_paused_sources: i64,
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
    pub opened_24h: i64,
    pub approved_24h: i64,
    pub rejected_24h: i64,
    /// The median time from opening to approval or rejection of the reviews decided in the last
    /// 7 days; `None` while none was.
    pub median_decision_secs_7d: Option<i64>,
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
    /// cannot finish.
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
            },
            throughput: Throughput {
                pages_scraped_1h: 120,
                pages_scraped_24h: 2_400,
                product_urls_found_24h: 35,
                crawls_finished_1h: 2,
                crawls_finished_24h: 30,
            },
            backlog: Backlog {
                pages_not_scraped_for_a_day: 500,
                pages_never_scraped: 60,
                rate_limited_sources: 1,
                scrape_paused_sources: 2,
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
                opened_24h: 14,
                approved_24h: 10,
                rejected_24h: 3,
                median_decision_secs_7d: Some(5_400),
            },
            agent_mode: true,
        };

        assert_eq!(
            serde_json::to_value(overview).unwrap_or_else(|error| panic!("{error}")),
            json!({
                "sources": {
                    "crawling_now": 4, "scraping_now": 11, "needs_attention": 23,
                    "crawl_failing": 2, "crawl_needs_operator": 6, "blocked_by_review": 9,
                    "failing_to_scrape": 7, "held_removals": 1, "identity_conflicts": 5,
                    "labelled": 3, "parked": 8, "stale": 12
                },
                "freshness": {
                    "stale_after_days": 3,
                    "since_last_scrape": {
                        "sources": 40, "median_secs": 3600, "mean_secs": 7200,
                        "p90_secs": 86400, "max_secs": 604800
                    },
                    "since_last_crawl": null
                },
                "throughput": {
                    "pages_scraped_1h": 120, "pages_scraped_24h": 2400,
                    "product_urls_found_24h": 35, "crawls_finished_1h": 2,
                    "crawls_finished_24h": 30
                },
                "backlog": {
                    "pages_not_scraped_for_a_day": 500, "pages_never_scraped": 60,
                    "rate_limited_sources": 1, "scrape_paused_sources": 2
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
                "review_flow": {
                    "opened_24h": 14, "approved_24h": 10, "rejected_24h": 3,
                    "median_decision_secs_7d": 5400
                },
                "agent_mode": true
            })
        );
    }
}
