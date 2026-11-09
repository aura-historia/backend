//! `GET /api/reviews`: the console's review queue, which the console polls while its tab is
//! visible. The contract is in `docs/crawler/operations.md` (review service).

use crate::CrawlerReviewId;
use crate::review::agent_decisions::{AgentDecisionPolicy, AgentRounds};
use crate::review::model::{AutoSchemaEvaluationOutcome, ReviewQueueEntry};
use crate::review::repository::{
    CrawlerReviewRepository, agent_repair_rounds, crawler_review_id_from_uuid,
    listing_source_id_from_uuid, row_to_repair_request,
};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

/// Reads the console's review queue under the review agents' decision policy of the start, which
/// decides whether a pending review waits for a human.
#[derive(Clone)]
pub struct ReviewQueueReader {
    pool: PgPool,
    agent_decisions: AgentDecisionPolicy,
}

impl ReviewQueueReader {
    /// Reads the database `repository` persists reviews to, under its review agents' decision
    /// policy, so the queue reports `human_required` as the repository enforces it.
    pub fn of(repository: &CrawlerReviewRepository) -> Self {
        Self {
            pool: repository.pool().clone(),
            agent_decisions: repository.agent_decisions(),
        }
    }

    /// At most `limit` reviews. Pending reviews of crawl-enabled ListingSources come first, newest
    /// first: a pending review of a disabled ListingSource holds back no crawling or scraping, so it
    /// follows them, ahead of the closed reviews. Of the validation summary only the LLM
    /// evaluation's outcome is read.
    pub async fn list_reviews(&self, limit: i64) -> Result<Vec<ReviewQueueEntry>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name,
                    COALESCE(s.crawl_enabled, FALSE) AS listing_source_crawl_enabled,
                    r.artifact_type, r.status, r.reason, r.candidate_version,
                    r.repair_requested, r.repair_note,
                    r.validation_summary -> 'auto_schema_evaluation' AS auto_schema_evaluation,
                    r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             ORDER BY
               CASE
                 WHEN r.status = 'PENDING_REVIEW' AND COALESCE(s.crawl_enabled, FALSE) THEN 0
                 WHEN r.status = 'PENDING_REVIEW' THEN 1
                 ELSE 2
               END,
               r.created DESC
             LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let review_ids = rows
            .iter()
            .map(|row| crawler_review_id_from_uuid(row.try_get("review_id")?))
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let agent_rounds =
            agent_repair_rounds(&self.pool, &self.agent_decisions, &review_ids).await?;
        rows.into_iter()
            .map(|row| self.queue_entry(row, &agent_rounds))
            .collect()
    }

    /// The queue row of `row`, with its agent rounds from `agent_rounds`. A stored evaluation that
    /// does not read as one fails the queue, as any other invalid persisted review does, with an
    /// error that names the review, so the console's log points at the row to fix.
    fn queue_entry(
        &self,
        row: PgRow,
        agent_rounds: &HashMap<CrawlerReviewId, AgentRounds>,
    ) -> Result<ReviewQueueEntry, sqlx::Error> {
        let review_id = crawler_review_id_from_uuid(row.try_get("review_id")?)?;
        let status: String = row.try_get("status")?;
        let rounds = agent_rounds.get(&review_id).copied().unwrap_or_default();
        let auto_schema_evaluation = row
            .try_get::<Option<serde_json::Value>, _>("auto_schema_evaluation")?
            .map(serde_json::from_value::<AutoSchemaEvaluationOutcome>)
            .transpose()
            .map_err(|source| {
                sqlx::Error::Decode(Box::new(UnreadableAutoSchemaEvaluation {
                    review_id,
                    source,
                }))
            })?;
        Ok(ReviewQueueEntry {
            review_id,
            listing_source_id: listing_source_id_from_uuid(row.try_get("listing_source_id")?)?,
            listing_source_name: row.try_get("listing_source_name")?,
            listing_source_crawl_enabled: row.try_get("listing_source_crawl_enabled")?,
            artifact_type: row.try_get("artifact_type")?,
            human_required: self.agent_decisions.waits_for_human(&status, rounds),
            status,
            reason: row.try_get("reason")?,
            candidate_version: row.try_get("candidate_version")?,
            repair_request: row_to_repair_request(&row)?,
            agent_rounds: rounds.needs_repair,
            auto_schema_evaluation,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
            reviewed: row.try_get("reviewed")?,
        })
    }
}

/// A review whose validation summary keeps an `auto_schema_evaluation` that is no evaluation the
/// crawler writes.
#[derive(Debug, thiserror::Error)]
#[error("review {review_id} keeps an unreadable LLM schema evaluation")]
struct UnreadableAutoSchemaEvaluation {
    review_id: CrawlerReviewId,
    #[source]
    source: serde_json::Error,
}
