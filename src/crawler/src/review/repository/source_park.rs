//! Retrying the ListingSources the review agents parked: once an agent's park label expires
//! ([`SourceLabel::expires_when_agent_set`]), the agents clear it and the source goes back to
//! qualification, so a shop that works now is crawled again and opens fresh reviews
//! (`docs/crawler/README.md#listingsource-scope`). A human's label never expires, and a human's
//! qualification of the source stands. Every clear is written to the source's label and
//! qualification history.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::source_label::{clear_label, persisted_label};
use super::source_qualification::clear_stored_qualification;
use super::{CrawlerReviewRepository, ReviewRepositoryError, listing_source_id_from_uuid};
#[cfg(doc)]
use crate::review::model::SourceLabel;
use crate::review::model::{RetriedParkedSource, ReviewActor};
use sqlx::Row;
use time::OffsetDateTime;

/// The most parked sources one retry sends back to qualification, so a backlog of expired parks
/// reaches the qualifier over several runs.
pub const MAX_PARK_RETRIES_PER_CALL: i64 = 100;

impl CrawlerReviewRepository {
    /// Clears the expired park labels as `actor`, the oldest expiry first and at most
    /// [`MAX_PARK_RETRIES_PER_CALL`], and sends their sources back to qualification: a source whose
    /// qualification no human decided waits for a new one, uncrawled; one a human qualified keeps
    /// that qualification, so the next scope refresh enables it. The reviewer agent retries
    /// nothing. Sources another transaction holds are left to the next call.
    pub async fn retry_parked_sources(
        &self,
        actor: ReviewActor,
    ) -> Result<Vec<RetriedParkedSource>, ReviewRepositoryError> {
        if !actor.may_change_candidates() {
            return Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates);
        }
        let mut transaction = self.pool.begin().await?;
        let expired = sqlx::query(
            "SELECT listing_source_id, listing_source_name, source_label, source_labelled_at, \
                    qualified_by IS DISTINCT FROM $1 AS requalifies, \
                    qualification IS NOT NULL AS qualified \
             FROM listing_sources \
             WHERE source_label_retry_at <= NOW() \
             ORDER BY source_label_retry_at, listing_source_id \
             LIMIT $2 \
             FOR UPDATE SKIP LOCKED",
        )
        .bind(ReviewActor::Human.as_str())
        .bind(MAX_PARK_RETRIES_PER_CALL)
        .fetch_all(&mut *transaction)
        .await?;
        let mut retried = Vec::with_capacity(expired.len());
        for row in expired {
            let source = RetriedParkedSource {
                listing_source_id: listing_source_id_from_uuid(row.try_get("listing_source_id")?)?,
                listing_source_name: row.try_get("listing_source_name")?,
                label: persisted_label(&row.try_get::<String, _>("source_label")?)?,
                labelled_at: row.try_get::<OffsetDateTime, _>("source_labelled_at")?,
                requalifies: row.try_get("requalifies")?,
            };
            clear_label(&mut transaction, &source.listing_source_id, actor).await?;
            if source.requalifies {
                if row.try_get::<bool, _>("qualified")? {
                    clear_stored_qualification(&mut transaction, &source.listing_source_id, actor)
                        .await?;
                }
                self.withhold_crawl(&mut transaction, &source.listing_source_id)
                    .await?;
            }
            retried.push(source);
        }
        transaction.commit().await?;
        for source in &retried {
            log_committed_source(
                &self.pool,
                &source.listing_source_id,
                ReviewLogEntry::park_retried(actor, source.label, source.requalifies),
            )
            .await;
        }
        Ok(retried)
    }
}
