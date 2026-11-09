//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::ListingSourceOverviewReader;
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{ConsoleOverview, PendingByArtifact, ReviewCounts, SourceCounts};
use crate::review::repository::{
    CrawlerReviewRepository, after_last_human_event, agent_handed_review_to_human,
    agent_round_binds, agent_rounds_of_review,
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

    /// The counts of all ListingSources and of all pending reviews: two queries, one for each.
    pub async fn overview(&self) -> Result<ConsoleOverview, sqlx::Error> {
        let health = self.sources.health_counts().await?;
        let reviews = self.review_counts().await?;
        Ok(ConsoleOverview {
            sources: SourceCounts {
                crawling_now: self.sources.running_count(WorkLockScope::Spider),
                scraping_now: self.sources.running_count(WorkLockScope::Scraper),
                needs_attention: health.needs_attention,
                crawl_needs_operator: health.crawl_needs_operator,
                blocked_by_review: health.blocked_by_review,
                failing_to_scrape: health.failing_to_scrape,
                held_removals: health.held_removals,
            },
            reviews,
            agent_mode: self.agent_decisions.repair_rounds_cap().is_some(),
        })
    }

    async fn review_counts(&self) -> Result<ReviewCounts, sqlx::Error> {
        let (agents, repair_requested, human) = agent_round_binds();
        let row = sqlx::query(REVIEW_COUNTS_SQL)
            .bind(agents)
            .bind(repair_requested)
            .bind(human)
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
        })
    }
}

/// The pending review counts. Binds `$1` to `$3` of [`agent_rounds_of_review`] and `$4`, the
/// repair-round cap or null.
const REVIEW_COUNTS_SQL: &str = concat!(
    "WITH pending AS (
         SELECT r.artifact_type, COALESCE(s.crawl_enabled, FALSE) AS crawl_enabled,
                $4::bigint IS NOT NULL AND (",
    agent_rounds_of_review!(),
    " >= $4 OR ",
    agent_handed_review_to_human!(),
    ") AS waits_for_human
         FROM crawler_reviews r
         LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
         WHERE r.status = 'PENDING_REVIEW'
     )
     SELECT COUNT(*) AS pending_total,
            COUNT(*) FILTER (WHERE artifact_type = 'URL_PATTERN') AS pending_url_pattern,
            COUNT(*) FILTER (WHERE artifact_type = 'PRODUCT_SCHEMA') AS pending_product_schema,
            COUNT(*) FILTER (WHERE NOT crawl_enabled) AS pending_disabled_sources,
            COUNT(*) FILTER (WHERE waits_for_human) AS pending_waiting_for_human
     FROM pending"
);
