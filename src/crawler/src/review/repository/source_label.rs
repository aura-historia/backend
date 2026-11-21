//! The label of a ListingSource the crawler should not crawl, and its removal
//! (`docs/crawler/README.md#listingsource-scope`). A human labels any source; in `agent` mode the
//! author agent labels the source of the pending review it works. Labelling switches crawling off;
//! the scope sync keeps it off while the label stands. Only a human clears a label, which also
//! qualifies the source (`docs/crawler/README.md#listingsource-qualification`). Every label
//! set or cleared appends one entry to the source's history in the same transaction, so a relabel
//! never loses the label and note it replaces.

use super::repair_rounds::{human_required, rounds_under_lock};
use super::review_log::{ReviewLogEntry, log_committed_source};
use super::source_qualification::qualify_after_label_clear;
use super::{CrawlerReviewRepository, ReviewRepositoryError, listing_source_id_from_uuid};
use crate::CrawlerReviewId;
use crate::review::agent_decisions::{AgentDecisionPolicy, FinalRound};
use crate::review::model::STATUS_PENDING_REVIEW;
use crate::review::model::{
    AppliedSourceLabel, ParkSchedule, Qualification, ReviewActor, SourceLabel,
    SourceLabelEventKind, SourceLabelNote, SourceLabelRequest,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgConnection, Postgres, Row, Transaction};
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
        let stored = store_label(
            &mut transaction,
            listing_source_id,
            actor,
            request,
            self.agent_decisions.park_schedule(),
        )
        .await?;
        transaction.commit().await?;
        Ok(self
            .labelled(listing_source_id, actor, request, stored)
            .await)
    }

    /// Labels the ListingSource of the review `review_id` as [`Self::label_source`] does, for the
    /// actor that may change the review: the author agent on a pending review that does not wait
    /// for a human or whose final round started, which parks the source, or a human. A review agent names its evidence in the note. The reviewer agent
    /// labels nothing, and the author agent replaces no label a human set. An approach label must
    /// name the approach of the review it is set on.
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
        let (listing_source_id, artifact_type) = lock_review_for_label(
            &mut transaction,
            review_id,
            actor,
            request.label(),
            &self.agent_decisions,
        )
        .await?;
        if let Some(required) = request.label().required_artifact()
            && required.as_str() != artifact_type
        {
            return Err(ReviewRepositoryError::SourceLabelDoesNotFitReview {
                label: request.label().as_str(),
                artifact_type,
            });
        }
        if actor.is_agent() && labelled_by_human(&mut transaction, &listing_source_id).await? {
            return Err(ReviewRepositoryError::HumanLabelStands(listing_source_id));
        }
        let stored = store_label(
            &mut transaction,
            &listing_source_id,
            actor,
            request,
            self.agent_decisions.park_schedule(),
        )
        .await?;
        transaction.commit().await?;
        Ok(self
            .labelled(&listing_source_id, actor, request, stored)
            .await)
    }

    /// Logs the committed label and answers it.
    async fn labelled(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        request: &SourceLabelRequest,
        stored: StoredLabel,
    ) -> AppliedSourceLabel {
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_labelled(actor, request.label(), request.note().is_some())
                .with_park(stored.park),
        )
        .await;
        AppliedSourceLabel {
            label: request.label(),
            note: request.note().map(|note| note.as_str().to_owned()),
            labelled_at: stored.labelled_at,
            labelled_by: actor,
            retry_at: stored.park.map(|park| park.retry_at),
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
            qualify_after_label_clear(
                &mut transaction,
                listing_source_id,
                actor,
                label,
                self.agent_decisions.park_schedule(),
            )
            .await?;
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

/// Locks the review `review_id` for `actor`'s label of its ListingSource and answers the source and
/// the review's artifact type. The reviewer agent labels nothing, and an agent labels only through
/// a pending review that does not wait for a human, or one whose final round started: the agents
/// park the source of a review their final round could not decide instead of leaving it for a
/// human. `AGENTS_UNRESOLVED` is an agent's label only in that final round, so a regular round
/// does not park a source the final round has not tried.
async fn lock_review_for_label(
    transaction: &mut Transaction<'_, Postgres>,
    review_id: CrawlerReviewId,
    actor: ReviewActor,
    label: SourceLabel,
    agent_decisions: &AgentDecisionPolicy,
) -> Result<(ListingSourceId, String), ReviewRepositoryError> {
    let row = sqlx::query(
        "SELECT listing_source_id, artifact_type, status \
         FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::NotFound(review_id))?;
    if !actor.may_change_candidates() {
        return Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates);
    }
    let status: String = row.try_get("status")?;
    if status != STATUS_PENDING_REVIEW && !actor.may_change_decided_reviews() {
        return Err(ReviewRepositoryError::AgentMayChangeOnlyPendingReviews(
            review_id,
        ));
    }
    if actor.is_agent() {
        let rounds = rounds_under_lock(transaction, review_id, agent_decisions).await?;
        if rounds.final_round == FinalRound::NotStarted
            && agent_decisions.waits_for_human(&status, rounds)
        {
            return Err(human_required(review_id, rounds, agent_decisions));
        }
        if label == SourceLabel::AgentsUnresolved && rounds.final_round == FinalRound::NotStarted {
            return Err(ReviewRepositoryError::FinalRoundOnlyLabel(review_id));
        }
    }
    Ok((
        listing_source_id_from_uuid(row.try_get::<uuid::Uuid, _>("listing_source_id")?)?,
        row.try_get("artifact_type")?,
    ))
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
             source_labelled_at = NULL, source_labelled_by = NULL, source_label_retry_at = NULL, \
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

/// Locks the ListingSource's row and answers whether a human set the label it carries, which only
/// a human replaces outside the qualification backfill; a ListingSource that does not exist is
/// refused.
async fn labelled_by_human(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
) -> Result<bool, ReviewRepositoryError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT source_labelled_by IS NOT DISTINCT FROM $2 \
         FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(ReviewActor::Human.as_str())
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::ListingSourceNotFound(
        *listing_source_id,
    ))
}

/// The label a stored name stands for; a name no label has fails closed.
pub(super) fn persisted_label(name: &str) -> Result<SourceLabel, sqlx::Error> {
    SourceLabel::from_persisted(name).ok_or_else(|| {
        sqlx::Error::Decode(format!("invalid persisted source_label `{name}`").into())
    })
}

/// A label as [`store_label`] stored it: when, and for an agent's park when the agents try the
/// source again.
#[derive(Debug, Clone, Copy)]
pub(super) struct StoredLabel {
    pub(super) labelled_at: OffsetDateTime,
    pub(super) park: Option<StoredPark>,
}

/// An agent's park of a ListingSource: the how-manyth since a human last labelled it or cleared its
/// label, and when it expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoredPark {
    pub(crate) parks: u32,
    pub(crate) retry_at: OffsetDateTime,
}

/// Stores `actor`'s label on the ListingSource with crawling switched off and appends it to the
/// source's label history, and answers when. A review agent's label that
/// [expires](SourceLabel::expires_when_agent_set) parks the source until `park`'s schedule tries it
/// again; any other label never expires. A ListingSource that does not exist is refused.
pub(super) async fn store_label(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    request: &SourceLabelRequest,
    park: ParkSchedule,
) -> Result<StoredLabel, ReviewRepositoryError> {
    let parks = if actor.is_agent() && request.label().expires_when_agent_set() {
        Some(
            agent_parks_since_human(transaction, listing_source_id)
                .await?
                .saturating_add(1),
        )
    } else {
        None
    };
    let retry_after = parks.map(|parks| park.label_retry_after(parks).as_seconds_f64());
    let (labelled_at, retry_at) = sqlx::query_as::<_, (OffsetDateTime, Option<OffsetDateTime>)>(
        "UPDATE listing_sources \
         SET source_label = $2, source_label_note = $3, \
             source_labelled_at = NOW(), source_labelled_by = $4, \
             source_label_retry_at = NOW() + make_interval(secs => $5::float8), \
             crawl_enabled = FALSE, updated = NOW() \
         WHERE listing_source_id = $1 \
         RETURNING source_labelled_at, source_label_retry_at",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(request.label().as_str())
    .bind(request.note().map(SourceLabelNote::as_str))
    .bind(actor.as_str())
    .bind(retry_after)
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
    Ok(StoredLabel {
        labelled_at,
        park: parks
            .zip(retry_at)
            .map(|(parks, retry_at)| StoredPark { parks, retry_at }),
    })
}

/// How many labels that expire review agents set on the ListingSource since a human last labelled
/// it or cleared its label: the parks before the one being set.
async fn agent_parks_since_human(
    connection: &mut PgConnection,
    listing_source_id: &ListingSourceId,
) -> Result<u32, sqlx::Error> {
    let expiring: Vec<&str> = SourceLabel::ALL
        .into_iter()
        .filter(|label| label.expires_when_agent_set())
        .map(SourceLabel::as_str)
        .collect();
    let parks = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM listing_source_label_events e \
         WHERE e.listing_source_id = $1 AND e.event_kind = 'LABELLED' \
           AND e.actor = ANY($2) AND e.source_label = ANY($3) \
           AND e.occurred_at > COALESCE( \
               (SELECT MAX(h.occurred_at) FROM listing_source_label_events h \
                WHERE h.listing_source_id = $1 AND h.actor = $4), \
               '-infinity'::timestamptz)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(ReviewActor::AGENTS.map(ReviewActor::as_str))
    .bind(&expiring)
    .bind(ReviewActor::Human.as_str())
    .fetch_one(connection)
    .await?;
    Ok(u32::try_from(parks).unwrap_or(u32::MAX))
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
