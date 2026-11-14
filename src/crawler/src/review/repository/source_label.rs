//! The label of a ListingSource the crawler should not crawl, and its removal
//! (`docs/crawler/README.md#listingsource-scope`). A human labels any source; in `agent` mode the
//! author agent labels the source of the pending review it works. Labelling switches crawling off;
//! the scope sync keeps it off while the label stands. Only a human clears a label, which also
//! qualifies the source (`docs/crawler/README.md#listingsource-qualification`). Every label
//! set or cleared appends one entry to the source's history in the same transaction, so a relabel
//! never loses the label and note it replaces.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::source_qualification::qualify_after_label_clear;
use super::{CrawlerReviewRepository, ReviewRepositoryError, lock_review_for_change};
use crate::CrawlerReviewId;
use crate::review::model::{
    AppliedSourceLabel, Qualification, ReviewActor, SourceLabel, SourceLabelEventKind,
    SourceLabelNote, SourceLabelRequest,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgConnection, Postgres, Transaction};
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
        let mut transaction = self.pool.begin().await?;
        let labelled_at = store_label(&mut transaction, listing_source_id, actor, request).await?;
        transaction.commit().await?;
        Ok(self
            .labelled(listing_source_id, actor, request, labelled_at)
            .await)
    }

    /// Labels the ListingSource of the review `review_id` as [`Self::label_source`] does, for the
    /// actor that may change the review: the author agent on a pending review that does not wait
    /// for a human, or a human. A review agent names its evidence in the note. The reviewer agent
    /// labels nothing. An approach label must name the approach of the review it is set on.
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
        if let Some(required) = request.label().required_artifact()
            && required.as_str() != review.artifact_type
        {
            return Err(ReviewRepositoryError::SourceLabelDoesNotFitReview {
                label: request.label().as_str(),
                artifact_type: review.artifact_type,
            });
        }
        let listing_source_id = review.listing_source_id;
        let labelled_at = store_label(&mut transaction, &listing_source_id, actor, request).await?;
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
    /// The clear qualifies the source, so it does not wait for a qualification again. Clearing a
    /// source without a label changes nothing and succeeds.
    pub async fn clear_source_label(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let cleared = clear_label(&mut transaction, listing_source_id, actor).await?;
        if let Some(label) = cleared {
            qualify_after_label_clear(&mut transaction, listing_source_id, actor, label).await?;
        }
        transaction.commit().await?;
        if cleared.is_some() {
            log_committed_source(
                &self.pool,
                listing_source_id,
                ReviewLogEntry::source_label_cleared(actor),
            )
            .await;
            log_committed_source(
                &self.pool,
                listing_source_id,
                ReviewLogEntry::source_qualified(actor, Qualification::Qualified, None),
            )
            .await;
        }
        Ok(())
    }
}

/// Clears the ListingSource's label with the crawl failure state the labelled shop left and
/// appends the clear to its label history, answering the label it cleared; `None` when the source
/// had none. A ListingSource that does not exist is refused, and a stored label no label has
/// fails closed.
pub(super) async fn clear_label(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
) -> Result<Option<SourceLabel>, ReviewRepositoryError> {
    let stored = sqlx::query_scalar::<_, Option<String>>(
        "SELECT source_label FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::ListingSourceNotFound(
        *listing_source_id,
    ))?;
    let Some(name) = stored else {
        return Ok(None);
    };
    let label = persisted_label(&name)?;
    sqlx::query(
        "UPDATE listing_sources \
         SET source_label = NULL, source_label_note = NULL, \
             source_labelled_at = NULL, source_labelled_by = NULL, \
             last_crawl_outcome = NULL, last_crawl_error_kind = NULL, \
             crawl_failure_count = 0, next_crawl_at = NULL, updated = NOW() \
         WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .execute(&mut **transaction)
    .await?;
    append_label_event(
        transaction,
        listing_source_id,
        SourceLabelEventKind::Cleared,
        None,
        actor,
    )
    .await?;
    Ok(Some(label))
}

/// The label a stored name stands for; a name no label has fails closed.
pub(super) fn persisted_label(name: &str) -> Result<SourceLabel, sqlx::Error> {
    SourceLabel::from_persisted(name).ok_or_else(|| {
        sqlx::Error::Decode(format!("invalid persisted source_label `{name}`").into())
    })
}

/// Stores `actor`'s label on the ListingSource with crawling switched off and appends it to the
/// source's label history, and answers when; a ListingSource that does not exist is refused.
pub(super) async fn store_label(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    request: &SourceLabelRequest,
) -> Result<OffsetDateTime, ReviewRepositoryError> {
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
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::ListingSourceNotFound(
        *listing_source_id,
    ))?;
    append_label_event(
        transaction,
        listing_source_id,
        SourceLabelEventKind::Labelled,
        Some(request),
        actor,
    )
    .await?;
    Ok(labelled_at)
}

/// Appends one entry to the ListingSource's label history: the label `request` sets, or a clear
/// without one.
async fn append_label_event(
    connection: &mut PgConnection,
    listing_source_id: &ListingSourceId,
    kind: SourceLabelEventKind,
    request: Option<&SourceLabelRequest>,
    actor: ReviewActor,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO listing_source_label_events \
             (listing_source_id, event_kind, source_label, source_label_note, actor) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(kind.as_str())
    .bind(request.map(|request| request.label().as_str()))
    .bind(
        request
            .and_then(SourceLabelRequest::note)
            .map(SourceLabelNote::as_str),
    )
    .bind(actor.as_str())
    .execute(connection)
    .await?;
    Ok(())
}
