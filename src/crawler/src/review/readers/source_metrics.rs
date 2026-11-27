//! The first page's numbers over all ListingSources (`GET /api/overview`), read from the
//! [overview rows](super::listing_source_overview) in their one pass over the product URLs. The
//! contract is in `docs/crawler/operations.md` (review service).

use super::ListingSourceOverviewReader;
use super::listing_source_overview::{OVERVIEW_CTES, RANKED_SELECT, order_by};
use crate::review::model::{
    AgeDistribution, CRAWL_RUN_HOURS, CrawlFreshness, FailureKindCount, FailureKinds, Freshness,
    HourlyCrawlRuns, IdleSource, LONGEST_IDLE_SOURCES, PageFreshness, STALE_AFTER_DAYS,
    SortDirection, SourceSort, Throughput,
};
use crate::review::repository::listing_source_id_from_uuid;
use crate::scraper::removal_guard::MASS_REMOVAL_HELD_ERROR_KIND;
#[cfg(doc)]
use crate::spider::candidate_service::SPIDER_RECRAWL_INTERVAL;
use crate::spider::candidate_service::{FAILED_CRAWL_OUTCOME, NEEDS_OPERATOR_CRAWL_OUTCOME};
use sqlx::Row;
use sqlx::postgres::PgRow;
use time::OffsetDateTime;

/// How many ListingSources are in each condition that needs an operator. The conditions overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCounts {
    pub needs_attention: i64,
    pub crawl_failing: i64,
    pub crawl_needs_operator: i64,
    pub blocked_by_review: i64,
    pub failing_to_scrape: i64,
    pub held_removals: i64,
    /// Sources with product URLs in an identity conflict.
    pub identity_conflicts: i64,
    /// Sources a human labelled, over all sources.
    pub labelled: i64,
    /// Sources the review agents parked until they try them again.
    pub parked: i64,
    /// Crawl-enabled sources the crawler has not worked for [`STALE_AFTER_DAYS`]; informational.
    pub stale: i64,
    /// Crawl-enabled sources whose shop's rate limit holds their next crawl back right now.
    pub rate_limited: i64,
    /// Crawl-enabled sources whose scraper circuit pauses their scrapes right now.
    pub scrape_paused: i64,
}

/// The first page's numbers over all ListingSources, read in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMetrics {
    pub health: HealthCounts,
    pub freshness: Freshness,
    pub throughput: Throughput,
    pub crawls: CrawlFreshness,
    pub pages: PageFreshness,
}

impl ListingSourceOverviewReader {
    /// The first page's numbers over all ListingSources, in one pass over their product URLs:
    /// how many are in each condition that needs an operator, how fresh the crawl-enabled ones
    /// and their pages are, the longest idle stale ones and the recent work.
    pub async fn source_metrics(&self) -> Result<SourceMetrics, sqlx::Error> {
        let row = sqlx::query(sqlx::AssertSqlSafe(source_metrics_sql()))
            .bind(None::<uuid::Uuid>)
            .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
            .bind(MASS_REMOVAL_HELD_ERROR_KIND)
            .fetch_one(self.pool())
            .await?;
        let pages_total: i64 = row.try_get("pages_total")?;
        let pages_never_scraped: i64 = row.try_get("pages_never_scraped")?;
        let pages_not_scraped_for_a_day: i64 = row.try_get("pages_not_scraped_for_a_day")?;
        let pages_overdue: i64 = row.try_get("pages_overdue")?;
        let crawl_sources: i64 = row.try_get("crawl_sources")?;
        let never_complete: i64 = row.try_get("never_complete")?;
        let complete_before: i64 = row.try_get("complete_before")?;
        Ok(SourceMetrics {
            health: HealthCounts {
                needs_attention: row.try_get("needs_attention")?,
                crawl_failing: row.try_get("crawl_failing")?,
                crawl_needs_operator: row.try_get("crawl_needs_operator")?,
                blocked_by_review: row.try_get("blocked_by_review")?,
                failing_to_scrape: row.try_get("failing_to_scrape")?,
                held_removals: row.try_get("held_removals")?,
                identity_conflicts: row.try_get("identity_conflicts")?,
                labelled: row.try_get("labelled")?,
                parked: row.try_get("parked")?,
                stale: row.try_get("stale")?,
                rate_limited: row.try_get("rate_limited")?,
                scrape_paused: row.try_get("scrape_paused")?,
            },
            freshness: Freshness {
                stale_after_days: STALE_AFTER_DAYS,
                since_last_scrape: age_distribution(&row, "scrape_age")?,
                since_last_crawl: age_distribution(&row, "crawl_age")?,
                longest_idle: longest_idle(&row)?,
            },
            throughput: Throughput {
                pages_scraped_1h: row.try_get("pages_scraped_1h")?,
                pages_scraped_24h: row.try_get("pages_scraped_24h")?,
                product_urls_found_24h: row.try_get("product_urls_found_24h")?,
                crawls_finished_1h: row.try_get("crawls_finished_1h")?,
                crawls_finished_24h: row.try_get("crawls_finished_24h")?,
                crawl_runs_by_hour: crawl_runs_by_hour(&row)?,
            },
            crawls: CrawlFreshness {
                total: crawl_sources,
                full_crawl_interval_days: FULL_CRAWL_INTERVAL_DAYS,
                complete: crawl_sources - never_complete - complete_before,
                complete_before,
                never_complete,
                last_run_truncated: row.try_get("last_run_truncated")?,
            },
            pages: PageFreshness {
                total: pages_total,
                scraped_within_a_day: pages_total
                    - pages_never_scraped
                    - pages_not_scraped_for_a_day,
                due: pages_not_scraped_for_a_day - pages_overdue,
                overdue: pages_overdue,
                never_scraped: pages_never_scraped,
            },
        })
    }

    /// What the crawl-enabled ListingSources that fail to scrape, and those whose last crawl
    /// failed, fail with, as the `failing` and `crawl_failing` lists count them.
    pub async fn failure_kinds(&self) -> Result<FailureKinds, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT 'scrape' AS work, last_scrape_error_kind AS error_kind, COUNT(*) AS sources
             FROM listing_sources
             WHERE crawl_enabled AND scrape_failure_streak > 0
             GROUP BY last_scrape_error_kind
             UNION ALL
             SELECT 'crawl', last_crawl_error_kind, COUNT(*)
             FROM listing_sources
             WHERE crawl_enabled AND last_crawl_outcome IS NOT DISTINCT FROM $1
             GROUP BY last_crawl_error_kind
             ORDER BY sources DESC, error_kind NULLS LAST",
        )
        .bind(FAILED_CRAWL_OUTCOME)
        .fetch_all(self.pool())
        .await?;
        let mut kinds = FailureKinds {
            scrape: Vec::new(),
            crawl: Vec::new(),
        };
        for row in &rows {
            let count = FailureKindCount {
                error_kind: row.try_get("error_kind")?,
                sources: row.try_get("sources")?,
            };
            match row.try_get::<&str, _>("work")? {
                "scrape" => kinds.scrape.push(count),
                _ => kinds.crawl.push(count),
            }
        }
        Ok(kinds)
    }
}

/// The one row of [`ListingSourceOverviewReader::source_metrics`], aggregated over the ranked
/// overview rows; binds `$1` to `$3` of [`OVERVIEW_CTES`]. It names `ranked` once, so Postgres
/// plans the URL pass for the columns read here only.
fn source_metrics_sql() -> String {
    format!(
        "{OVERVIEW_CTES}
         ), ranked AS ({RANKED_SELECT})
         SELECT COUNT(*) FILTER (WHERE needs_attention) AS needs_attention,
                COUNT(*) FILTER (WHERE crawl_enabled AND crawl_failed) AS crawl_failing,
                COUNT(*) FILTER (WHERE crawl_enabled AND needs_operator) AS crawl_needs_operator,
                COUNT(*) FILTER (WHERE crawl_enabled AND blocked_urls > 0) AS blocked_by_review,
                COUNT(*) FILTER (WHERE crawl_enabled AND scrape_failure_streak > 0)
                    AS failing_to_scrape,
                COUNT(*) FILTER (WHERE crawl_enabled AND held_removal_urls > 0) AS held_removals,
                COUNT(*) FILTER (WHERE crawl_enabled AND identity_conflict_urls > 0)
                    AS identity_conflicts,
                COUNT(*) FILTER (WHERE source_label IS NOT NULL) AS labelled,
                COUNT(*) FILTER (WHERE source_label_retry_at IS NOT NULL
                                    OR qualification_retry_at IS NOT NULL) AS parked,
                COUNT(*) FILTER (WHERE stale) AS stale,
                COUNT(*) FILTER (WHERE crawl_enabled AND crawl_rate_limited_until > NOW())
                    AS rate_limited,
                COUNT(*) FILTER (WHERE crawl_enabled AND scrape_paused_until > NOW())
                    AS scrape_paused,
                {scrape_ages},
                {crawl_ages},
                {longest_idle},
                COALESCE(SUM(scraped_1h), 0)::BIGINT AS pages_scraped_1h,
                COALESCE(SUM(product_urls - never_scraped - not_scraped_for_a_day), 0)::BIGINT
                    AS pages_scraped_24h,
                COALESCE(SUM(found_24h), 0)::BIGINT AS product_urls_found_24h,
                COUNT(*) FILTER (WHERE last_crawl_run_at > NOW() - INTERVAL '1 hour')
                    AS crawls_finished_1h,
                COUNT(*) FILTER (WHERE last_crawl_run_at > NOW() - INTERVAL '24 hours')
                    AS crawls_finished_24h,
                {THIS_HOUR} AS this_hour,
                {crawl_runs},
                COALESCE(SUM(product_urls) FILTER (WHERE crawl_enabled), 0)::BIGINT
                    AS pages_total,
                COALESCE(SUM(never_scraped) FILTER (WHERE crawl_enabled), 0)::BIGINT
                    AS pages_never_scraped,
                COALESCE(SUM(not_scraped_for_a_day) FILTER (WHERE crawl_enabled), 0)::BIGINT
                    AS pages_not_scraped_for_a_day,
                COALESCE(SUM(overdue_pages) FILTER (WHERE crawl_enabled), 0)::BIGINT
                    AS pages_overdue,
                COUNT(*) FILTER (WHERE crawl_enabled) AS crawl_sources,
                COUNT(*) FILTER (WHERE crawl_enabled AND last_complete_crawl IS NULL)
                    AS never_complete,
                COUNT(*) FILTER (WHERE crawl_enabled
                                   AND last_complete_crawl
                                       <= NOW() - INTERVAL '{FULL_CRAWL_INTERVAL_DAYS} days')
                    AS complete_before,
                COUNT(*) FILTER (WHERE crawl_enabled AND last_run_truncated)
                    AS last_run_truncated
         FROM ranked",
        scrape_ages = age_aggregates("last_successful_scrape", "scrape_age"),
        crawl_ages = age_aggregates("last_crawl_run_at", "crawl_age"),
        longest_idle = longest_idle_aggregates(),
        crawl_runs = crawl_run_aggregates(),
    )
}

/// How many days apart the spider's full crawls are due ([`SPIDER_RECRAWL_INTERVAL`]).
const FULL_CRAWL_INTERVAL_DAYS: i64 = 7;

/// The start of the running clock hour. Hours are cut in UTC whatever the session's time zone.
const THIS_HOUR: &str = "date_trunc('hour', NOW(), 'UTC')";

/// One count per clock hour of [`Throughput::crawl_runs_by_hour`], `crawl_runs_0` for the
/// oldest hour: the sources whose last spider run ended in it. They aggregate the overview
/// rows, one per source, not the URLs.
fn crawl_run_aggregates() -> String {
    (0..CRAWL_RUN_HOURS)
        .map(|hour| {
            format!(
                "COUNT(*) FILTER (WHERE date_trunc('hour', last_crawl_run_at, 'UTC')
                                      = {THIS_HOUR} - INTERVAL '{hours_ago} hours')
                     AS crawl_runs_{hour}",
                hours_ago = CRAWL_RUN_HOURS - 1 - hour,
            )
        })
        .collect::<Vec<_>>()
        .join(",\n")
}

/// The hours [`crawl_run_aggregates`] counted, the oldest first.
fn crawl_runs_by_hour(row: &PgRow) -> Result<[HourlyCrawlRuns; CRAWL_RUN_HOURS], sqlx::Error> {
    let this_hour: OffsetDateTime = row.try_get("this_hour")?;
    let mut hours = [HourlyCrawlRuns {
        hour: this_hour,
        sources: 0,
    }; CRAWL_RUN_HOURS];
    let mut hours_ago = i64::try_from(CRAWL_RUN_HOURS).unwrap_or(i64::MAX);
    for (index, runs) in hours.iter_mut().enumerate() {
        hours_ago -= 1;
        runs.hour = this_hour - time::Duration::hours(hours_ago);
        runs.sources = row.try_get(format!("crawl_runs_{index}").as_str())?;
    }
    Ok(hours)
}

/// The first [`LONGEST_IDLE_SOURCES`] stale sources in the order of the `stale` list sorted by
/// `idle_since` ascending, as three arrays of the same order: `longest_idle_ids`,
/// `longest_idle_names` and `longest_idle_since`; null while no source is stale.
fn longest_idle_aggregates() -> String {
    let order = order_by(SourceSort::IdleSince, SortDirection::Ascending);
    let first = |column: &str, alias: &str| {
        format!(
            "(array_agg({column} ORDER BY {order}) FILTER (WHERE stale))[1:{LONGEST_IDLE_SOURCES}]
                 AS {alias}"
        )
    };
    [
        first("listing_source_id", "longest_idle_ids"),
        first("listing_source_name", "longest_idle_names"),
        first("idle_since", "longest_idle_since"),
    ]
    .join(",\n")
}

/// The sources [`longest_idle_aggregates`] read.
fn longest_idle(row: &PgRow) -> Result<Vec<IdleSource>, sqlx::Error> {
    let ids: Option<Vec<uuid::Uuid>> = row.try_get("longest_idle_ids")?;
    let names: Option<Vec<Option<String>>> = row.try_get("longest_idle_names")?;
    let since: Option<Vec<Option<OffsetDateTime>>> = row.try_get("longest_idle_since")?;
    ids.unwrap_or_default()
        .into_iter()
        .zip(names.unwrap_or_default())
        .zip(since.unwrap_or_default())
        .map(|((id, listing_source_name), idle_since)| {
            Ok(IdleSource {
                listing_source_id: listing_source_id_from_uuid(id)?,
                listing_source_name,
                idle_since,
            })
        })
        .collect()
}

/// The aggregates of the ages of `column`, in whole seconds, over the crawl-enabled rows that
/// have one: `{alias}_sources`, `{alias}_median`, `{alias}_mean`, `{alias}_p90` and `{alias}_max`.
/// Both arguments are static SQL fragments.
fn age_aggregates(column: &str, alias: &str) -> String {
    let age = format!("EXTRACT(EPOCH FROM NOW() - {column})::FLOAT8");
    let only = format!("FILTER (WHERE crawl_enabled AND {column} IS NOT NULL)");
    format!(
        "COUNT(*) {only} AS {alias}_sources,
         (percentile_cont(0.5) WITHIN GROUP (ORDER BY {age}) {only})::BIGINT AS {alias}_median,
         (AVG({age}) {only})::BIGINT AS {alias}_mean,
         (percentile_cont(0.9) WITHIN GROUP (ORDER BY {age}) {only})::BIGINT AS {alias}_p90,
         (MAX({age}) {only})::BIGINT AS {alias}_max"
    )
}

/// The distribution [`age_aggregates`] read under `alias`; `None` while no row had an age.
fn age_distribution(row: &PgRow, alias: &str) -> Result<Option<AgeDistribution>, sqlx::Error> {
    let sources: i64 = row.try_get(format!("{alias}_sources").as_str())?;
    if sources == 0 {
        return Ok(None);
    }
    let seconds = |name: &str| -> Result<i64, sqlx::Error> {
        row.try_get::<Option<i64>, _>(format!("{alias}_{name}").as_str())?
            .ok_or_else(|| sqlx::Error::Decode(format!("{alias}_{name} without a value").into()))
    };
    Ok(Some(AgeDistribution {
        sources,
        median_secs: seconds("median")?,
        mean_secs: seconds("mean")?,
        p90_secs: seconds("p90")?,
        max_secs: seconds("max")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::candidate_service::SPIDER_RECRAWL_INTERVAL;

    #[test]
    fn should_count_complete_crawls_over_the_spiders_full_recrawl_interval() {
        assert_eq!(
            u64::try_from(FULL_CRAWL_INTERVAL_DAYS).unwrap_or_default() * 24 * 60 * 60,
            SPIDER_RECRAWL_INTERVAL.as_secs()
        );
    }

    #[test]
    fn should_count_the_crawl_runs_of_every_hour_up_to_the_running_one() {
        let aggregates = crawl_run_aggregates();

        assert_eq!(
            aggregates.matches("AS crawl_runs_").count(),
            CRAWL_RUN_HOURS
        );
        assert!(
            aggregates.contains("INTERVAL '23 hours')\n                     AS crawl_runs_0"),
            "the oldest hour first: {aggregates}"
        );
        assert!(
            aggregates.contains("INTERVAL '0 hours')\n                     AS crawl_runs_23"),
            "the running hour last: {aggregates}"
        );
    }

    #[test]
    fn should_name_the_longest_idle_sources_in_the_order_of_the_stale_list() {
        let aggregates = longest_idle_aggregates();

        assert_eq!(
            aggregates
                .matches(&format!(
                    "ORDER BY {}) FILTER (WHERE stale))[1:{LONGEST_IDLE_SOURCES}]",
                    order_by(SourceSort::IdleSince, SortDirection::Ascending)
                ))
                .count(),
            3,
            "ids, names and times in the list's order: {aggregates}"
        );
    }
}
