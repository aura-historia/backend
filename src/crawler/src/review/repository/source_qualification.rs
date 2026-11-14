//! The qualification of a ListingSource as a real antique e-shop, decided before it is crawled
//! (`docs/crawler/README.md#listingsource-qualification`). In `agent` mode the author agent
//! decides each source of the qualification queue once; a human decides or clears any source's.
//! A decision that names a label sets it as a label is set, replacing the source's label, which
//! stays in its label history. A review agent never clears a label: an agent qualification of a
//! labelled source without a label keeps the label for a human. Every decision and clear appends
//! one entry to the source's qualification history in its transaction.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::source_label::{clear_label, persisted_label, store_label};
use super::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::review::model::{
    AppliedQualification, PreviousSourceLabel, Qualification, QualificationDecision,
    QualificationLabelEffect, QualificationOutcome, ReviewActor, SourceLabel, SourceLabelNote,
    SourceLabelRequest,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgConnection, Postgres, Row, Transaction};
use time::OffsetDateTime;

/// What a decision reads of its source under the source's row lock.
struct SourceUnderQualification {
    in_crawl_scope: bool,
    qualification: Option<String>,
    label: Option<PreviousSourceLabel>,
    backfill: bool,
}

impl CrawlerReviewRepository {
    /// Decides the ListingSource's qualification as `actor`. A review agent decides only as the
    /// author, only a source the scope admits that waits for its qualification, and replaces a
    /// label a human set only in the one-off backfill of a source that existed before
    /// qualification did. The decision's label replaces the source's label; a human's
    /// qualification without a label clears it, an agent's keeps it for a human.
    pub async fn decide_qualification(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
        decision: &QualificationDecision,
    ) -> Result<QualificationOutcome, ReviewRepositoryError> {
        if actor == ReviewActor::AgentReviewer {
            return Err(ReviewRepositoryError::ReviewerMayNotChangeCandidates);
        }
        let mut transaction = self.pool.begin().await?;
        let source = lock_source(&mut transaction, listing_source_id).await?;
        if actor.is_agent() {
            check_agent_may_decide(listing_source_id, &source)?;
        }
        let (label_effect, label) = match (decision.label(), &source.label) {
            (Some(label), _) => {
                let request = SourceLabelRequest::new(label, Some(decision.note().clone()))?;
                store_label(&mut transaction, listing_source_id, actor, &request).await?;
                (QualificationLabelEffect::Labelled, Some(label))
            }
            (None, Some(_)) if decision.qualification() == Qualification::Qualified => {
                if actor.is_agent() {
                    (
                        QualificationLabelEffect::KeptLabel,
                        source.label.as_ref().map(|previous| previous.label),
                    )
                } else {
                    clear_label(&mut transaction, listing_source_id, actor).await?;
                    (QualificationLabelEffect::ClearedLabel, None)
                }
            }
            (None, previous) => (
                QualificationLabelEffect::None,
                previous.as_ref().map(|previous| previous.label),
            ),
        };
        let qualified_at = store_qualification(
            &mut transaction,
            listing_source_id,
            actor,
            decision.qualification(),
            decision.label(),
            decision.note(),
        )
        .await?;
        transaction.commit().await?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_qualified(actor, decision.qualification(), decision.label()),
        )
        .await;
        Ok(QualificationOutcome {
            applied: AppliedQualification {
                qualification: decision.qualification(),
                note: decision.note().as_str().to_owned(),
                qualified_at,
                qualified_by: actor,
            },
            label_effect,
            label,
            previous_label: source.label,
        })
    }

    /// Clears the ListingSource's qualification, so the qualification queue offers it again; its
    /// label, if any, stays. Clearing a source without a qualification changes nothing and
    /// succeeds.
    pub async fn clear_qualification(
        &self,
        listing_source_id: &ListingSourceId,
        actor: ReviewActor,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let source = lock_source(&mut transaction, listing_source_id).await?;
        if source.qualification.is_none() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE listing_sources \
             SET qualification = NULL, qualification_note = NULL, qualified_at = NULL, \
                 qualified_by = NULL, updated = NOW() \
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .execute(&mut *transaction)
        .await?;
        append_qualification_event(&mut transaction, listing_source_id, None, None, None, actor)
            .await?;
        transaction.commit().await?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_qualification_cleared(actor),
        )
        .await;
        Ok(())
    }
}

/// Refuses a review agent's decision on a source that is not its to decide.
fn check_agent_may_decide(
    listing_source_id: &ListingSourceId,
    source: &SourceUnderQualification,
) -> Result<(), ReviewRepositoryError> {
    if source.qualification.is_some() {
        return Err(ReviewRepositoryError::SourceAlreadyQualified(
            *listing_source_id,
        ));
    }
    if !source.in_crawl_scope {
        return Err(ReviewRepositoryError::SourceNotInCrawlScope(
            *listing_source_id,
        ));
    }
    let human_label = source
        .label
        .as_ref()
        .is_some_and(|label| label.labelled_by == ReviewActor::Human);
    if human_label && !source.backfill {
        return Err(ReviewRepositoryError::HumanLabelStands(*listing_source_id));
    }
    Ok(())
}

/// Locks the ListingSource's row and reads what a decision depends on; a stored label or actor
/// no label or actor has fails closed.
async fn lock_source(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
) -> Result<SourceUnderQualification, ReviewRepositoryError> {
    let row = sqlx::query(
        "SELECT in_crawl_scope, qualification, source_label, source_labelled_by, \
                qualification_backfill \
         FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(ReviewRepositoryError::ListingSourceNotFound(
        *listing_source_id,
    ))?;
    let label = match (
        row.try_get::<Option<String>, _>("source_label")?,
        row.try_get::<Option<String>, _>("source_labelled_by")?,
    ) {
        (Some(label), Some(actor)) => Some(PreviousSourceLabel {
            label: persisted_label(&label)?,
            labelled_by: ReviewActor::from_persisted(&actor).ok_or_else(|| {
                sqlx::Error::Decode(format!("invalid persisted actor `{actor}`").into())
            })?,
        }),
        _ => None,
    };
    Ok(SourceUnderQualification {
        in_crawl_scope: row.try_get("in_crawl_scope")?,
        qualification: row.try_get("qualification")?,
        label,
        backfill: row.try_get("qualification_backfill")?,
    })
}

/// Stores `actor`'s qualification on the ListingSource and appends it to the source's
/// qualification history, answering when. The first decision ends the source's part in the
/// one-off backfill: should its qualification be cleared, it waits uncrawled like a new source,
/// and no agent replaces a label a human set on it.
async fn store_qualification(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    qualification: Qualification,
    label: Option<SourceLabel>,
    note: &SourceLabelNote,
) -> Result<OffsetDateTime, ReviewRepositoryError> {
    let qualified_at = sqlx::query_scalar::<_, OffsetDateTime>(
        "UPDATE listing_sources \
         SET qualification = $2, qualification_note = $3, qualified_at = NOW(), \
             qualified_by = $4, qualification_backfill = FALSE, updated = NOW() \
         WHERE listing_source_id = $1 \
         RETURNING qualified_at",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(qualification.as_str())
    .bind(note.as_str())
    .bind(actor.as_str())
    .fetch_one(&mut **transaction)
    .await?;
    append_qualification_event(
        transaction,
        listing_source_id,
        Some(qualification),
        label,
        Some(note),
        actor,
    )
    .await?;
    Ok(qualified_at)
}

/// Qualifies the ListingSource whose label `actor` just cleared, so a cleared source is crawled
/// again without waiting for a qualification; the note names the cleared label.
pub(super) async fn qualify_after_label_clear(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    cleared: SourceLabel,
) -> Result<(), ReviewRepositoryError> {
    let note = SourceLabelNote::try_from(format!(
        "Qualified by clearing the label {}.",
        cleared.as_str()
    ))?;
    store_qualification(
        transaction,
        listing_source_id,
        actor,
        Qualification::Qualified,
        None,
        &note,
    )
    .await?;
    Ok(())
}

/// Appends one entry to the ListingSource's qualification history: a decision, or a clear
/// without one.
async fn append_qualification_event(
    connection: &mut PgConnection,
    listing_source_id: &ListingSourceId,
    qualification: Option<Qualification>,
    label: Option<SourceLabel>,
    note: Option<&SourceLabelNote>,
    actor: ReviewActor,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO listing_source_qualification_events \
             (listing_source_id, qualification, source_label, note, actor) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(qualification.map(Qualification::as_str))
    .bind(label.map(SourceLabel::as_str))
    .bind(note.map(SourceLabelNote::as_str))
    .bind(actor.as_str())
    .execute(connection)
    .await?;
    Ok(())
}
