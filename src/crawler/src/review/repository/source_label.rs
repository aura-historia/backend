//! The label of a ListingSource the crawler should not crawl, and its removal
//! (`docs/crawler/README.md#listingsource-scope`). A human labels any source; in `agent` mode the
//! author agent labels the source of the pending review it works. Labelling switches crawling off;
//! the scope sync keeps it off while the label stands. Only a human clears a label.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::{CrawlerReviewRepository, ReviewRepositoryError, lock_review_for_change};
use crate::CrawlerReviewId;
use crate::review::model::{AppliedSourceLabel, ReviewActor, SourceLabelNote, SourceLabelRequest};
use listing_source_core::ListingSourceId;
use sqlx::PgExecutor;
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
        let labelled_at = store_label(&self.pool, listing_source_id, actor, request)
            .await?
            .ok_or(ReviewRepositoryError::ListingSourceNotFound(
                *listing_source_id,
            ))?;
        Ok(self
            .labelled(listing_source_id, actor, request, labelled_at)
            .await)
    }

    /// Labels the ListingSource of the review `review_id` as [`Self::label_source`] does, for the
    /// actor that may change the review: the author agent on a pending review that does not wait
    /// for a human, or a human. A review agent names its evidence in the note. The reviewer agent
    /// labels nothing.
    pub async fn label_review_source(
        &self,
        review_id: CrawlerReviewId,
        actor: ReviewActor,
        request: &SourceLabelRequest,
    ) -> Result<AppliedSourceLabel, ReviewRepositoryError> {
        if actor.is_agent() && request.note().is_none() {
            return Err(ReviewRepositoryError::AgentSourceLabelNeedsNote);
        }
        let mut transaction = self.pool.begin().await?;
        let (review, _) =
            lock_review_for_change(&mut transaction, review_id, actor, &self.agent_decisions)
                .await?;
        let listing_source_id = review.listing_source_id;
        let labelled_at = store_label(&mut *transaction, &listing_source_id, actor, request)
            .await?
            .ok_or(ReviewRepositoryError::ListingSourceNotFound(
                listing_source_id,
            ))?;
        transaction.commit().await?;
        Ok(self
            .labelled(&listing_source_id, actor, request, labelled_at)
            .await)
    }

    /// Logs the committed label and answers it.
    async fn labelled(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        request: &SourceLabelRequest,
        labelled_at: OffsetDateTime,
    ) -> AppliedSourceLabel {
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_labelled(actor, request.label(), request.note().is_some()),
        )
        .await;
        AppliedSourceLabel {
            label: request.label(),
            note: request.note().map(|note| note.as_str().to_owned()),
            labelled_at,
            labelled_by: actor,
        }
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

/// Stores `actor`'s label on the ListingSource with crawling switched off, and answers when, or
/// `None` when no such ListingSource exists.
async fn store_label<'e>(
    executor: impl PgExecutor<'e>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    request: &SourceLabelRequest,
) -> Result<Option<OffsetDateTime>, sqlx::Error> {
    sqlx::query_scalar::<_, OffsetDateTime>(
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
    .fetch_optional(executor)
    .await
}
