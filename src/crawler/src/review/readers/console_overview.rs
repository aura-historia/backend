//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{ConsoleOverview, PendingReviewCounts, SourceCounts};
use crate::review::repository::{
    CrawlerReviewRepository, after_last_human_event, agent_handed_review_to_human,
    agent_round_binds, agent_rounds_of_review,
};
use crate::spider::candidate_service::NEEDS_OPERATOR_CRAWL_OUTCOME;
use crate::spider::local_lock::{LocalLockManager, WorkLockScope};
use sqlx::{PgPool, Row};
use std::sync::Arc;

/// Reads the counts of the console's first page under the review agents' decision policy of the
/// start, which decides whether a pending review waits for a human.
#[derive(Clone)]
pub struct ConsoleOverviewReader {
    pool: PgPool,
    agent_decisions: AgentDecisionPolicy,
    work_locks: Arc<LocalLockManager>,
}

impl ConsoleOverviewReader {
    /// Reads the database `repository` persists reviews to, under its review agents' decision
    /// policy, and reports no running work, as a console without a crawler in its process has.
    pub fn of(repository: &CrawlerReviewRepository) -> Self {
        Self {
            pool: repository.pool().clone(),
            agent_decisions: repository.agent_decisions(),
            work_locks: Arc::new(LocalLockManager::new()),
        }
    }

    /// This reader reporting the crawler's running work from `work_locks`, the lock manager the
    /// spider and scraper of this process hold their work locks in.
    pub fn with_work_locks(self, work_locks: Arc<LocalLockManager>) -> Self {
        Self { work_locks, ..self }
    }

    /// The counts of all ListingSources and of all pending reviews, in one query.
    pub async fn overview(&self) -> Result<ConsoleOverview, sqlx::Error> {
        let running = self.work_locks.running_work();
        let running_ids = |scope| {
            running
                .iter()
                .filter(|work| work.scope == scope)
                .map(|work| uuid::Uuid::from(work.listing_source_id))
                .collect::<Vec<_>>()
        };
        let (agents, repair_requested, human) = agent_round_binds();
        let row = sqlx::query(OVERVIEW_SQL)
            .bind(agents)
            .bind(repair_requested)
            .bind(human)
            .bind(
                self.agent_decisions
                    .repair_rounds_cap()
                    .map(|cap| i64::from(cap.get())),
            )
            .bind(NEEDS_OPERATOR_CRAWL_OUTCOME)
            .bind(running_ids(WorkLockScope::Spider))
            .bind(running_ids(WorkLockScope::Scraper))
            .fetch_one(&self.pool)
            .await?;
        Ok(ConsoleOverview {
            sources: SourceCounts {
                total: row.try_get("sources_total")?,
                crawl_enabled: row.try_get("sources_crawl_enabled")?,
                needs_operator: row.try_get("sources_needs_operator")?,
                crawling_now: row.try_get("sources_crawling_now")?,
                scraping_now: row.try_get("sources_scraping_now")?,
            },
            pending_reviews: PendingReviewCounts {
                total: row.try_get("pending_total")?,
                url_pattern: row.try_get("pending_url_pattern")?,
                product_schema: row.try_get("pending_product_schema")?,
                waiting_for_human: row.try_get("pending_waiting_for_human")?,
            },
        })
    }
}

/// The counts query. Binds `$1` to `$3` of [`agent_rounds_of_review`], `$4`, the repair-round cap
/// or null, `$5`, the crawl outcome that waits for an operator, and `$6` and `$7`, the
/// ListingSources the spider and the scraper of this process work right now.
const OVERVIEW_SQL: &str = concat!(
    "WITH pending AS (
         SELECT r.artifact_type,
                $4::bigint IS NOT NULL AND (",
    agent_rounds_of_review!(),
    " >= $4 OR ",
    agent_handed_review_to_human!(),
    ") AS waits_for_human
         FROM crawler_reviews r
         WHERE r.status = 'PENDING_REVIEW'
     )
     SELECT (SELECT COUNT(*) FROM listing_sources) AS sources_total,
            (SELECT COUNT(*) FROM listing_sources WHERE crawl_enabled) AS sources_crawl_enabled,
            (SELECT COUNT(*) FROM listing_sources
             WHERE crawl_enabled AND last_crawl_outcome = $5) AS sources_needs_operator,
            (SELECT COUNT(*) FROM listing_sources
             WHERE listing_source_id = ANY($6)) AS sources_crawling_now,
            (SELECT COUNT(*) FROM listing_sources
             WHERE listing_source_id = ANY($7)) AS sources_scraping_now,
            COUNT(*) AS pending_total,
            COUNT(*) FILTER (WHERE artifact_type = 'URL_PATTERN') AS pending_url_pattern,
            COUNT(*) FILTER (WHERE artifact_type = 'PRODUCT_SCHEMA') AS pending_product_schema,
            COUNT(*) FILTER (WHERE waits_for_human) AS pending_waiting_for_human
     FROM pending"
);
