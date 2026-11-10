//! `GET /api/reviews` and `GET /api/listing-sources/{id}/reviews`: the console's review queue, one
//! window at a time, which the console polls while its tab is visible. The contract is in
//! `docs/crawler/operations.md` (review service). Every filter, the search, the order and the
//! count run in SQL over all reviews.

use crate::CrawlerReviewId;
use crate::review::agent_decisions::AgentDecisionPolicy;
use crate::review::model::{
    ArtifactType, AutoSchemaEvaluationOutcome, ListPage, ListWindow, ReviewActor, ReviewQueueEntry,
    ReviewSort, ReviewStatus, ReviewWaiting, SearchTerm, SortDirection, SourceReviewEntry,
};
use crate::review::repository::{
    CrawlerReviewRepository, after_last_human_event, agent_handed_review_to_human,
    agent_round_binds, agent_rounds_of_review, crawler_review_id_from_uuid,
    listing_source_id_from_uuid, row_to_repair_request,
};
use listing_source_core::ListingSourceId;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

/// Which window of the review queue to read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReviewQuery {
    /// Reviews of a ListingSource whose name contains this text.
    pub search: Option<SearchTerm>,
    /// Only the reviews of this ListingSource.
    pub listing_source_id: Option<ListingSourceId>,
    pub status: Option<ReviewStatus>,
    pub artifact_type: Option<ArtifactType>,
    pub waiting: Option<ReviewWaiting>,
    pub sort: ReviewSort,
    pub direction: SortDirection,
    pub window: ListWindow,
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

    /// The window of reviews `query` asks for, with how many reviews match it in all. Of the
    /// validation summary only the LLM evaluation's outcome is read.
    pub async fn list_reviews(
        &self,
        query: &ReviewQuery,
    ) -> Result<ListPage<ReviewQueueEntry>, sqlx::Error> {
        let (agents, repair_requested, human, window_days) =
            agent_round_binds(&self.agent_decisions);
        let rows = sqlx::query(sqlx::AssertSqlSafe(review_page_sql(
            query.sort,
            query.direction,
        )))
        .bind(agents)
        .bind(repair_requested)
        .bind(human)
        .bind(window_days)
        .bind(
            self.agent_decisions
                .repair_rounds_cap()
                .map(|cap| i64::from(cap.get())),
        )
        .bind(query.listing_source_id.map(|id| *id.as_uuid()))
        .bind(query.status.map(ReviewStatus::as_str))
        .bind(query.artifact_type.map(ArtifactType::as_str))
        .bind(query.search.as_ref().map(SearchTerm::contains_pattern))
        .bind(query.waiting.map(waiting_name))
        .bind(i64::try_from(query.window.offset()).unwrap_or(i64::MAX))
        .bind(i64::try_from(query.window.limit()).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;

        let Some(total) = rows.first().map(|row| row.try_get("total")).transpose()? else {
            return Err(sqlx::Error::RowNotFound);
        };
        let mut reviews = Vec::with_capacity(rows.len());
        for row in &rows {
            if row.try_get::<Option<uuid::Uuid>, _>("review_id")?.is_some() {
                reviews.push(queue_entry(row)?);
            }
        }
        Ok(ListPage::new(reviews, total, query.window))
    }

    /// The window of the reviews of `listing_source_id` that `query` asks for, newest first and of
    /// both artifact types unless `query` filters, each with whether it is the live one and who
    /// changed it last, read for the whole window in one more query.
    pub async fn list_source_reviews(
        &self,
        listing_source_id: ListingSourceId,
        query: &ReviewQuery,
    ) -> Result<ListPage<SourceReviewEntry>, sqlx::Error> {
        let page = self
            .list_reviews(&ReviewQuery {
                listing_source_id: Some(listing_source_id),
                sort: ReviewSort::Created,
                direction: SortDirection::Descending,
                ..query.clone()
            })
            .await?;
        let review_ids = page
            .items
            .iter()
            .map(|review| *review.review_id.as_uuid())
            .collect::<Vec<_>>();
        let mut context = self.source_review_context(&review_ids).await?;
        let items = page
            .items
            .into_iter()
            .map(|review| {
                let context = context.remove(&review.review_id).unwrap_or_default();
                SourceReviewEntry {
                    review,
                    candidate_is_live: context.candidate_is_live,
                    last_actor: context.last_actor,
                    rejection_revoked: context.rejection_revoked,
                }
            })
            .collect();
        Ok(ListPage {
            items,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// Whether each of `review_ids` is the latest approved review of its ListingSource and
    /// artifact type, the actor of its latest event and whether its rejection was revoked.
    async fn source_review_context(
        &self,
        review_ids: &[uuid::Uuid],
    ) -> Result<HashMap<CrawlerReviewId, SourceReviewContext>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.review_id,
                    r.status = 'APPROVED'
                      AND r.review_id = (
                          SELECT a.review_id FROM crawler_reviews a
                          WHERE a.listing_source_id = r.listing_source_id
                            AND a.artifact_type = r.artifact_type
                            AND a.status = 'APPROVED'
                          ORDER BY a.reviewed DESC NULLS LAST, a.review_id DESC
                          LIMIT 1) AS candidate_is_live,
                    (SELECT e.actor FROM crawler_review_events e
                     WHERE e.review_id = r.review_id
                     ORDER BY e.created DESC, e.event_id DESC
                     LIMIT 1) AS last_actor,
                    EXISTS (SELECT 1 FROM crawler_review_events e
                            WHERE e.review_id = r.review_id
                              AND e.action = 'REJECTION_REVOKED') AS rejection_revoked
             FROM crawler_reviews r
             WHERE r.review_id = ANY($1)",
        )
        .bind(review_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let actor = row
                    .try_get::<Option<String>, _>("last_actor")?
                    .map(|name| {
                        ReviewActor::from_persisted(&name).ok_or_else(|| {
                            sqlx::Error::Decode(format!("invalid persisted actor `{name}`").into())
                        })
                    })
                    .transpose()?;
                Ok((
                    crawler_review_id_from_uuid(row.try_get("review_id")?)?,
                    SourceReviewContext {
                        candidate_is_live: row.try_get("candidate_is_live")?,
                        last_actor: actor,
                        rejection_revoked: row.try_get("rejection_revoked")?,
                    },
                ))
            })
            .collect()
    }
}

/// What a ListingSource's review list says about a review beyond the queue entry.
#[derive(Default)]
struct SourceReviewContext {
    candidate_is_live: bool,
    last_actor: Option<ReviewActor>,
    rejection_revoked: bool,
}

fn waiting_name(waiting: ReviewWaiting) -> &'static str {
    match waiting {
        ReviewWaiting::Human => "human",
        ReviewWaiting::Repair => "repair",
    }
}

/// The queue row of `row`. A stored evaluation that does not read as one fails the queue, as any
/// other invalid persisted review does, with an error that names the review, so the console's log
/// points at the row to fix.
fn queue_entry(row: &PgRow) -> Result<ReviewQueueEntry, sqlx::Error> {
    let review_id = crawler_review_id_from_uuid(row.try_get("review_id")?)?;
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
        status: row.try_get("status")?,
        reason: row.try_get("reason")?,
        repair_request: row_to_repair_request(row)?,
        human_required: row.try_get("human_required")?,
        auto_schema_evaluation,
        created: row.try_get("created")?,
        updated: row.try_get("updated")?,
    })
}

/// The window query of the reviews in the order of `sort`. Binds `$1` to `$4` of
/// [`agent_rounds_of_review`], `$5`, the repair-round cap or null, `$6`, the ListingSource or null,
/// `$7`, the status or null, `$8`, the artifact type or null, `$9`, the `ILIKE` pattern of the
/// ListingSource name or null, `$10`, what the review waits for or null, `$11`, the rows to skip,
/// and `$12`, the most rows to answer. Every row carries the total; an empty window is one row
/// with the total and no review. Only a pending review's agent rounds are counted.
///
/// The SQL is assembled from static fragments only, chosen by `sort` and `direction`, never from
/// request text.
fn review_page_sql(sort: ReviewSort, direction: SortDirection) -> String {
    let order = review_order(sort, direction.sql());
    format!(
        "WITH candidates AS (
             SELECT r.review_id, r.listing_source_id, s.listing_source_name,
                    COALESCE(s.crawl_enabled, FALSE) AS listing_source_crawl_enabled,
                    r.artifact_type, r.status, r.reason,
                    r.repair_requested, r.repair_note,
                    r.validation_summary -> 'auto_schema_evaluation' AS auto_schema_evaluation,
                    r.created, r.updated,
                    CASE WHEN r.status = 'PENDING_REVIEW' THEN {rounds} ELSE 0 END AS agent_rounds,
                    CASE WHEN r.status = 'PENDING_REVIEW' THEN {handed} ELSE FALSE END
                        AS handed_to_human
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             WHERE ($6::uuid IS NULL OR r.listing_source_id = $6)
               AND ($7::text IS NULL OR r.status = $7)
               AND ($8::text IS NULL OR r.artifact_type = $8)
               AND ($9::text IS NULL OR s.listing_source_name ILIKE $9)
         ), scored AS (
             SELECT *, $5::bigint IS NOT NULL AND status = 'PENDING_REVIEW'
                           AND (agent_rounds >= $5 OR handed_to_human) AS human_required
             FROM candidates
         ), matching AS (
             SELECT * FROM scored
             WHERE $10::text IS NULL
                OR ($10 = 'human' AND human_required)
                OR ($10 = 'repair' AND status = 'PENDING_REVIEW'
                    AND repair_requested IS NOT NULL AND NOT human_required)
         ), ranked AS (
             SELECT matching.*, ROW_NUMBER() OVER (ORDER BY {order}) AS position FROM matching
         )
         SELECT t.total, p.*
         FROM (SELECT COUNT(*) AS total FROM matching) t
         LEFT JOIN (SELECT * FROM ranked WHERE position > $11 AND position <= $11 + $12) p ON TRUE
         ORDER BY p.position",
        rounds = agent_rounds_of_review!(),
        handed = agent_handed_review_to_human!(),
    )
}

/// The `ORDER BY` list of `sort` in `direction` (an SQL keyword). The queue order ignores the
/// direction.
fn review_order(sort: ReviewSort, direction: &str) -> String {
    match sort {
        ReviewSort::Queue => {
            "CASE WHEN status = 'PENDING_REVIEW' AND listing_source_crawl_enabled THEN 0
                 WHEN status = 'PENDING_REVIEW' THEN 1 ELSE 2 END,
            CASE WHEN status = 'PENDING_REVIEW' THEN created END ASC, created DESC, review_id"
                .to_owned()
        }
        ReviewSort::Created => format!("created {direction}, review_id"),
        ReviewSort::Updated => format!("updated {direction}, review_id"),
        ReviewSort::Source => {
            format!("lower(listing_source_name) {direction} NULLS LAST, created DESC, review_id")
        }
    }
}

/// A review whose validation summary keeps an `auto_schema_evaluation` that is no evaluation the
/// crawler writes.
#[derive(Debug, thiserror::Error)]
#[error("review {review_id} keeps an unreadable LLM schema evaluation")]
pub(super) struct UnreadableAutoSchemaEvaluation {
    pub(super) review_id: CrawlerReviewId,
    #[source]
    source: serde_json::Error,
}
