//! A human's label of a ListingSource the crawler should not crawl, and its removal
//! (`docs/crawler/README.md#listingsource-scope`). Labelling switches crawling off; the scope sync
//! keeps it off while the label stands.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::model::{AppliedSourceLabel, ReviewActor, SourceLabelNote, SourceLabelRequest};
use listing_source_core::ListingSourceId;
use time::OffsetDateTime;

impl CrawlerReviewRepository {
    /// Labels the ListingSource and switches its crawling off, replacing any earlier label. What
    /// the source holds stays: its URLs, schemas, frontier and reviews.
    pub async fn label_source(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        request: &SourceLabelRequest,
    ) -> Result<AppliedSourceLabel, ReviewRepositoryError> {
        let labelled_at = sqlx::query_scalar::<_, OffsetDateTime>(
            "UPDATE listing_sources \
             SET source_label = $2, source_label_note = $3, \
                 source_labelled_at = NOW(), source_labelled_by = $4, \
                 crawl_enabled = FALSE, updated = NOW() \
             WHERE listing_source_id = $1 \
             RETURNING source_labelled_at",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(request.label().as_str())
        .bind(request.note().map(SourceLabelNote::as_str))
        .bind(actor.as_str())
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::ListingSourceNotFound(
            *listing_source_id,
        ))?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_labelled(actor, request.label(), request.note().is_some()),
        )
        .await;
        Ok(AppliedSourceLabel {
            label: request.label(),
            note: request.note().map(|note| note.as_str().to_owned()),
            labelled_at,
            labelled_by: actor,
        })
    }

    /// Removes the label and the crawl failure state the labelled shop left, so the source starts
    /// a new failure streak and is due at once when the next scope refresh enables it again. It
    /// does not enable crawling itself, and keeps `last_crawled`, the frontier, URLs and schemas.
    /// Clearing a source without a label changes nothing and succeeds.
    pub async fn clear_source_label(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<(), ReviewRepositoryError> {
        let (found, cleared) = sqlx::query_as::<_, (bool, bool)>(
            "WITH cleared AS ( \
                 UPDATE listing_sources \
                 SET source_label = NULL, source_label_note = NULL, \
                     source_labelled_at = NULL, source_labelled_by = NULL, \
                     last_crawl_outcome = NULL, last_crawl_error_kind = NULL, \
                     crawl_failure_count = 0, next_crawl_at = NULL, updated = NOW() \
                 WHERE listing_source_id = $1 AND source_label IS NOT NULL \
                 RETURNING 1) \
             SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1), \
                    EXISTS (SELECT 1 FROM cleared)",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .fetch_one(&self.pool)
        .await?;
        if !found {
            return Err(ReviewRepositoryError::ListingSourceNotFound(
                *listing_source_id,
            ));
        }
        if cleared {
            log_committed_source(
                &self.pool,
                listing_source_id,
                ReviewLogEntry::source_label_cleared(actor),
            )
            .await;
        }
        Ok(())
    }
}
