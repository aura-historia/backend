//! A human's label that a ListingSource cannot be scraped, and its removal
//! (`docs/crawler/README.md#listingsource-scope`). Marking switches crawling off; the scope sync
//! keeps it off while the mark stands.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::model::{ReviewActor, UnscrapableMark, UnscrapableNote};
use listing_source_core::ListingSourceId;
use serde::Serialize;
use time::OffsetDateTime;

/// A stored mark as `POST /api/listing-sources/{id}/unscrapable` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnscrapableLabel {
    pub unscrapable_reason: &'static str,
    pub unscrapable_note: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub unscrapable_since: OffsetDateTime,
    pub unscrapable_by: ReviewActor,
}

impl CrawlerReviewRepository {
    /// Marks the ListingSource as unscrapable and switches its crawling off, replacing any earlier
    /// mark. What the source holds stays: its URLs, schemas and frontier.
    pub async fn mark_unscrapable(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        mark: &UnscrapableMark,
    ) -> Result<UnscrapableLabel, ReviewRepositoryError> {
        let since = sqlx::query_scalar::<_, OffsetDateTime>(
            "UPDATE listing_sources \
             SET unscrapable_reason = $2, unscrapable_note = $3, \
                 unscrapable_since = NOW(), unscrapable_by = $4, \
                 crawl_enabled = FALSE, updated = NOW() \
             WHERE listing_source_id = $1 \
             RETURNING unscrapable_since",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(mark.reason().as_str())
        .bind(mark.note().map(UnscrapableNote::as_str))
        .bind(actor.as_str())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::ListingSourceNotFound(
            *listing_source_id,
        ))?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::marked_unscrapable(actor, mark.reason(), mark.note().is_some()),
        )
        .await;
        Ok(UnscrapableLabel {
            unscrapable_reason: mark.reason().as_str(),
            unscrapable_note: mark.note().map(|note| note.as_str().to_owned()),
            unscrapable_since: since,
            unscrapable_by: actor,
        })
    }

    /// Removes the mark and resets the failure escalation the unscrapable shop accumulated, so
    /// the source is due again at once once the scope sync enables it. It does not enable
    /// crawling itself, and keeps `last_crawled`, the frontier, URLs and schemas. Clearing a
    /// source without a mark changes nothing and succeeds.
    pub async fn clear_unscrapable(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<(), ReviewRepositoryError> {
        let (found, cleared) = sqlx::query_as::<_, (bool, i64)>(
            "WITH cleared AS ( \
                 UPDATE listing_sources \
                 SET unscrapable_reason = NULL, unscrapable_note = NULL, \
                     unscrapable_since = NULL, unscrapable_by = NULL, \
                     last_crawl_outcome = NULL, last_crawl_error_kind = NULL, \
                     next_crawl_at = NULL, crawl_rate_limited_until = NULL, \
                     crawl_failure_count = 0, crawl_rate_limit_streak = 0, \
                     updated = NOW() \
                 WHERE listing_source_id = $1 AND unscrapable_reason IS NOT NULL \
                 RETURNING 1) \
             SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1), \
                    (SELECT COUNT(*) FROM cleared)",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .fetch_one(&self.pool)
        .await?;
        if !found {
            return Err(ReviewRepositoryError::ListingSourceNotFound(
                *listing_source_id,
            ));
        }
        if cleared == 0 {
            return Ok(());
        }
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::unscrapable_cleared(actor),
        )
        .await;
        Ok(())
    }
}
