//! `GET /api/reviews`: the console's review queue, page by page, which the console polls while its
//! tab is visible. The contract is in `docs/crawler/operations.md` (review service).
//!
//! A page starts after the review the previous one ended with, by that review's creation time and
//! id, which never change, so a review that closes or comes to wait for a human between two pages
//! moves no other review across the page boundary. Every filter, the search and the count run in
//! SQL over all reviews.

use super::PageReadError;
use crate::CrawlerReviewId;
use crate::review::agent_decisions::{AgentDecisionPolicy, AgentRounds};
use crate::review::model::{
    ArtifactType, AutoSchemaEvaluationOutcome, PageSize, ReviewQueueEntry, ReviewQueuePage,
    ReviewStatus, SearchTerm, SortDirection,
};
use crate::review::repository::{
    CrawlerReviewRepository, after_last_human_event, agent_handed_review_to_human,
    agent_round_binds, agent_rounds_of_review, crawler_review_id_from_uuid,
    listing_source_id_from_uuid, row_to_repair_request,
};
use listing_source_core::ListingSourceId;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

/// Which page of the review queue to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewQuery {
    /// Reviews whose ListingSource's name, slug or crawl host contains this text, or whose id is
    /// this text.
    pub search: Option<SearchTerm>,
    /// Only the reviews of this ListingSource.
    pub listing_source_id: Option<ListingSourceId>,
    /// Only reviews in one of these statuses; empty lists every status.
    pub statuses: Vec<ReviewStatus>,
    pub artifact_type: Option<ArtifactType>,
    /// Only the reviews whose `human_required` is this; `None` lists both.
    pub human_required: Option<bool>,
    /// The direction of the creation time: newest first by default.
    pub direction: SortDirection,
    /// The review the previous page ended with; `None` reads the first page.
    pub after: Option<CrawlerReviewId>,
    pub page_size: PageSize,
}

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

    /// The page of reviews `query` asks for, with how many reviews match it in all. A review that
    /// `query.after` names but that does not exist is [`PageReadError::UnknownCursor`]. Of the
    /// validation summary only the LLM evaluation's outcome is read.
    pub async fn list_reviews(
        &self,
        query: &ReviewQuery,
    ) -> Result<ReviewQueuePage, PageReadError> {
        let cursor = match query.after {
            Some(after) => Some(self.page_cursor(after).await?),
            None => None,
        };
        let page_size = query.page_size.get();
        let (agents, repair_requested, human) = agent_round_binds();
        let rows = sqlx::query(sqlx::AssertSqlSafe(review_page_sql(query.direction)))
            .bind(agents)
            .bind(repair_requested)
            .bind(human)
            .bind(
                self.agent_decisions
                    .repair_rounds_cap()
                    .map(|cap| i64::from(cap.get())),
            )
            .bind(query.listing_source_id.map(|id| *id.as_uuid()))
            .bind(
                query
                    .statuses
                    .iter()
                    .map(|status| status.as_str())
                    .collect::<Vec<_>>(),
            )
            .bind(query.artifact_type.map(ArtifactType::as_str))
            .bind(query.search.as_ref().map(SearchTerm::contains_pattern))
            .bind(
                query
                    .search
                    .as_ref()
                    .and_then(|term| term.as_str().parse::<CrawlerReviewId>().ok())
                    .map(|id| *id.as_uuid()),
            )
            .bind(query.human_required)
            .bind(cursor.map(|(created, _)| created))
            .bind(cursor.map(|(_, review_id)| *review_id.as_uuid()))
            .bind(i64::try_from(page_size.saturating_add(1)).unwrap_or(i64::MAX))
            .fetch_all(&self.pool)
            .await?;

        let total = match rows.first() {
            Some(row) => row.try_get("total")?,
            None => return Err(sqlx::Error::RowNotFound.into()),
        };
        let mut reviews = Vec::with_capacity(rows.len());
        for row in &rows {
            if row.try_get::<Option<uuid::Uuid>, _>("review_id")?.is_some() {
                reviews.push(self.queue_entry(row)?);
            }
        }
        let next_after = if reviews.len() > page_size {
            reviews.truncate(page_size);
            reviews.last().map(|review| review.review_id)
        } else {
            None
        };
        Ok(ReviewQueuePage {
            reviews,
            total,
            next_after,
        })
    }

    /// The creation time and id of the review `after`, which a page starts after.
    async fn page_cursor(
        &self,
        after: CrawlerReviewId,
    ) -> Result<(OffsetDateTime, CrawlerReviewId), PageReadError> {
        sqlx::query_scalar::<_, OffsetDateTime>(
            "SELECT created FROM crawler_reviews WHERE review_id = $1",
        )
        .bind(after.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .map(|created| (created, after))
        .ok_or_else(|| PageReadError::UnknownCursor(after.to_string()))
    }

    /// The queue row of `row`, with its agent rounds. A stored evaluation that does not read as
    /// one fails the queue, as any other invalid persisted review does, with an error that names
    /// the review, so the console's log points at the row to fix.
    fn queue_entry(&self, row: &PgRow) -> Result<ReviewQueueEntry, sqlx::Error> {
        let review_id = crawler_review_id_from_uuid(row.try_get("review_id")?)?;
        let status: String = row.try_get("status")?;
        // A count beyond `u32` is past any cap, so saturating keeps the review with a human.
        let rounds = AgentRounds {
            needs_repair: u32::try_from(row.try_get::<i64, _>("agent_rounds")?).unwrap_or(u32::MAX),
            handed_to_human: row.try_get("handed_to_human")?,
        };
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
            repair_request: row_to_repair_request(row)?,
            agent_rounds: rounds.needs_repair,
            auto_schema_evaluation,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
            reviewed: row.try_get("reviewed")?,
        })
    }
}

/// The page query of the reviews in `direction` of their creation time. Binds `$1` to `$3` of
/// [`agent_rounds_of_review`], `$4`, the repair-round cap or null, `$5`, the ListingSource or null,
/// `$6`, the statuses, `$7`, the artifact type or null, `$8` and `$9`, the search's `ILIKE` pattern
/// and the review id it names, or null, `$10`, the `human_required` filter or null, `$11` and
/// `$12`, the creation time and id of the review the page starts after or null, and `$13`, the
/// most rows to answer. Every row carries the total; an empty page is one row with the total and
/// no review. Only a pending review's agent rounds are counted.
///
/// The SQL is assembled from static fragments only, chosen by `direction`, never from request text.
fn review_page_sql(direction: SortDirection) -> String {
    let (order, after) = match direction {
        SortDirection::Ascending => ("ASC", ">"),
        SortDirection::Descending => ("DESC", "<"),
    };
    format!(
        "WITH candidates AS (
             SELECT r.review_id, r.listing_source_id, s.listing_source_name,
                    COALESCE(s.crawl_enabled, FALSE) AS listing_source_crawl_enabled,
                    r.artifact_type, r.status, r.reason, r.candidate_version,
                    r.repair_requested, r.repair_note,
                    r.validation_summary -> 'auto_schema_evaluation' AS auto_schema_evaluation,
                    r.created, r.updated, r.reviewed,
                    CASE WHEN r.status = 'PENDING_REVIEW' THEN {rounds} ELSE 0 END AS agent_rounds,
                    CASE WHEN r.status = 'PENDING_REVIEW' THEN {handed} ELSE FALSE END
                        AS handed_to_human
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             WHERE ($5::uuid IS NULL OR r.listing_source_id = $5)
               AND (cardinality($6::text[]) = 0 OR r.status = ANY($6))
               AND ($7::text IS NULL OR r.artifact_type = $7)
               AND ($8::text IS NULL
                    OR s.listing_source_name ILIKE $8
                    OR s.listing_source_slug ILIKE $8
                    OR s.crawl_host ILIKE $8
                    OR r.review_id::text ILIKE $8
                    OR r.review_id = $9::uuid)
         ), matching AS (
             SELECT * FROM candidates
             WHERE $10::boolean IS NULL
                OR $10 = ($4::bigint IS NOT NULL AND status = 'PENDING_REVIEW'
                          AND (agent_rounds >= $4 OR handed_to_human))
         ), page AS (
             SELECT * FROM matching
             WHERE $11::timestamptz IS NULL OR (created, review_id) {after} ($11, $12::uuid)
             ORDER BY created {order}, review_id {order}
             LIMIT $13
         )
         SELECT t.total, p.*
         FROM (SELECT COUNT(*) AS total FROM matching) t
         LEFT JOIN page p ON TRUE
         ORDER BY p.created {order}, p.review_id {order}",
        rounds = agent_rounds_of_review!(),
        handed = agent_handed_review_to_human!(),
    )
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
