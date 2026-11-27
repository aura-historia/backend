//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::ListingSourceOverviewReader;
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{
    ConsoleOverview, DailyReviewFlow, PendingByArtifact, REVIEW_FLOW_DAYS, ReviewCounts,
    ReviewFlow, SourceCounts,
};
use crate::review::repository::{
    CrawlerReviewRepository, agent_round_binds, held_for_non_product_loop, review_waits_for_human,
};
use crate::spider::local_lock::WorkLockScope;
use sqlx::{PgPool, Row};

/// Reads the counts of the console's first page under the review agents' decision policy of the
/// start, which decides whether a pending review waits for a human.
#[derive(Clone)]
pub struct ConsoleOverviewReader {
    pool: PgPool,
    agent_decisions: AgentDecisionPolicy,
    sources: ListingSourceOverviewReader,
}

impl ConsoleOverviewReader {
    /// Reads the database `repository` persists reviews to, under its review agents' decision
    /// policy, and the ListingSources and running work `sources` reports.
    pub fn new(repository: &CrawlerReviewRepository, sources: ListingSourceOverviewReader) -> Self {
        Self {
            pool: repository.pool().clone(),
            agent_decisions: repository.agent_decisions(),
            sources,
        }
    }

    /// The numbers of all ListingSources and of all reviews: one pass over the ListingSources and
    /// their URLs, one over their failures, one over the pending reviews and one over the recent
    /// decisions.
    pub async fn overview(&self) -> Result<ConsoleOverview, sqlx::Error> {
        let metrics = self.sources.source_metrics().await?;
        let failures = self.sources.failure_kinds().await?;
        let reviews = self.review_counts().await?;
        let review_flow = self.review_flow().await?;
        let health = metrics.health;
        Ok(ConsoleOverview {
            sources: SourceCounts {
                crawling_now: self.sources.running_count(WorkLockScope::Spider),
                scraping_now: self.sources.running_count(WorkLockScope::Scraper),
                needs_attention: health.needs_attention,
                crawl_failing: health.crawl_failing,
                crawl_needs_operator: health.crawl_needs_operator,
                blocked_by_review: health.blocked_by_review,
                failing_to_scrape: health.failing_to_scrape,
                held_removals: health.held_removals,
                identity_conflicts: health.identity_conflicts,
                labelled: health.labelled,
                parked: health.parked,
                stale: health.stale,
                rate_limited: health.rate_limited,
                scrape_paused: health.scrape_paused,
            },
            freshness: metrics.freshness,
            throughput: metrics.throughput,
            pages: metrics.pages,
            failures,
            reviews,
            review_flow,
            agent_mode: self.agent_decisions.repair_rounds_cap().is_some(),
        })
    }

    async fn review_counts(&self) -> Result<ReviewCounts, sqlx::Error> {
        let (agents, repair_requested, human, window_days) =
            agent_round_binds(&self.agent_decisions);
        let row = sqlx::query(REVIEW_COUNTS_SQL)
            .bind(agents)
            .bind(repair_requested)
            .bind(human)
            .bind(window_days)
            .bind(
                self.agent_decisions
                    .repair_rounds_cap()
                    .map(|cap| i64::from(cap.get())),
            )
            .fetch_one(&self.pool)
            .await?;
        Ok(ReviewCounts {
            pending_total: row.try_get("pending_total")?,
            pending_by_artifact: PendingByArtifact {
                product_schema: row.try_get("pending_product_schema")?,
                url_pattern: row.try_get("pending_url_pattern")?,
            },
            pending_disabled_sources: row.try_get("pending_disabled_sources")?,
            waiting_for_human: row.try_get("pending_waiting_for_human")?,
            parked: row.try_get("pending_parked")?,
            held_for_non_product_loop: row.try_get("pending_held_for_non_product_loop")?,
            oldest_pending: row.try_get("oldest_pending")?,
            median_pending_age_secs: row.try_get("median_pending_age_secs")?,
        })
    }

    async fn review_flow(&self) -> Result<ReviewFlow, sqlx::Error> {
        let row = sqlx::query(sqlx::AssertSqlSafe(review_flow_sql()))
            .fetch_one(&self.pool)
            .await?;
        let mut daily = [DailyReviewFlow::default(); REVIEW_FLOW_DAYS];
        for (day, flow) in daily.iter_mut().enumerate() {
            *flow = DailyReviewFlow {
                opened: row.try_get(format!("opened_{day}").as_str())?,
                approved: row.try_get(format!("approved_{day}").as_str())?,
                rejected: row.try_get(format!("rejected_{day}").as_str())?,
            };
        }
        Ok(ReviewFlow {
            daily,
            median_decision_secs_7d: row.try_get("median_decision_secs_7d")?,
        })
    }
}

/// The reviews opened and decided in each of the last [`REVIEW_FLOW_DAYS`] 24-hour windows,
/// `_0` the oldest and the last one ending now, in one pass over the reviews of those days. An
/// approval or rejection records when it decided (`reviewed`); a superseded review was not
/// decided on its own and is left out. The SQL is assembled from static fragments only.
fn review_flow_sql() -> String {
    let days = (0..REVIEW_FLOW_DAYS)
        .map(|day| {
            let window = |column: &str| {
                format!(
                    "{column} > NOW() - INTERVAL '{from} hours'
                     AND {column} <= NOW() - INTERVAL '{until} hours'",
                    from = 24 * (REVIEW_FLOW_DAYS - day),
                    until = 24 * (REVIEW_FLOW_DAYS - 1 - day),
                )
            };
            format!(
                "COUNT(*) FILTER (WHERE {opened}) AS opened_{day},
                 COUNT(*) FILTER (WHERE status = 'APPROVED' AND {decided}) AS approved_{day},
                 COUNT(*) FILTER (WHERE status = 'REJECTED' AND {decided}) AS rejected_{day}",
                opened = window("created"),
                decided = window("reviewed"),
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "SELECT {days},
                (percentile_cont(0.5)
                     WITHIN GROUP (ORDER BY EXTRACT(EPOCH FROM reviewed - created)::FLOAT8)
                     FILTER (WHERE status IN ('APPROVED', 'REJECTED')
                               AND reviewed > NOW() - INTERVAL '7 days'))::BIGINT
                    AS median_decision_secs_7d
         FROM crawler_reviews
         WHERE created > NOW() - INTERVAL '7 days' OR reviewed > NOW() - INTERVAL '7 days'"
    )
}

/// The pending review counts. Binds `$1` to `$4` of [`agent_rounds_of_review`] and `$5`, the
/// repair-round cap or null.
const REVIEW_COUNTS_SQL: &str = concat!(
    "WITH pending AS (
         SELECT r.artifact_type, r.created, COALESCE(s.crawl_enabled, FALSE) AS crawl_enabled,
                ",
    review_waits_for_human!(),
    " AS waits_for_human, ",
    held_for_non_product_loop!(),
    " AS held_for_non_product_loop,
                s.source_label_retry_at IS NOT NULL OR s.qualification_retry_at IS NOT NULL
                    AS parked
         FROM crawler_reviews r
         LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
         WHERE r.status = 'PENDING_REVIEW'
     )
     SELECT COUNT(*) AS pending_total,
            COUNT(*) FILTER (WHERE artifact_type = 'URL_PATTERN') AS pending_url_pattern,
            COUNT(*) FILTER (WHERE artifact_type = 'PRODUCT_SCHEMA') AS pending_product_schema,
            COUNT(*) FILTER (WHERE NOT crawl_enabled) AS pending_disabled_sources,
            COUNT(*) FILTER (WHERE waits_for_human AND crawl_enabled) AS pending_waiting_for_human,
            COUNT(*) FILTER (WHERE parked) AS pending_parked,
            COUNT(*) FILTER (WHERE held_for_non_product_loop) AS pending_held_for_non_product_loop,
            MIN(created) FILTER (WHERE crawl_enabled) AS oldest_pending,
            (percentile_cont(0.5) WITHIN GROUP (ORDER BY EXTRACT(EPOCH FROM NOW() - created)::FLOAT8)
                FILTER (WHERE crawl_enabled))::BIGINT AS median_pending_age_secs
     FROM pending"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_end_the_last_review_flow_window_now() {
        let sql = review_flow_sql();

        assert_eq!(sql.matches(" AS opened_").count(), REVIEW_FLOW_DAYS);
        assert!(
            sql.contains("created > NOW() - INTERVAL '168 hours'")
                && sql.contains("created <= NOW() - INTERVAL '144 hours') AS opened_0"),
            "the oldest window first: {sql}"
        );
        assert!(
            sql.contains("created <= NOW() - INTERVAL '0 hours') AS opened_6"),
            "the last window ends now: {sql}"
        );
    }
}
