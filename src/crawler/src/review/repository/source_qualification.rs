//! The qualification of a ListingSource as a real art and antiques e-shop, decided before it is
//! crawled (`docs/crawler/README.md#listingsource-qualification`). In `agent` mode the author agent
//! decides each source of the qualification queue once; a human decides or clears any source's. A
//! decision that names a label sets it as a label is set, replacing the source's label, which stays
//! in its label history. A review agent never clears a label: an agent qualification of a labelled
//! source without a label keeps the label for a human. Every decision and clear appends one entry
//! to the source's qualification history in its transaction.

use super::review_log::{ReviewLogEntry, log_committed_source};
use super::source_label::{clear_label, persisted_label, store_label};
use super::verdicts::check_agent_decision_rate;
use super::{CrawlerReviewRepository, ReviewRepositoryError};
#[cfg(doc)]
use crate::review::model::AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND;
use crate::review::model::{
    AppliedQualification, ParkSchedule, PreviousSourceLabel, Qualification, QualificationDecision,
    QualificationLabelEffect, QualificationOutcome, ReviewActor, SourceLabel, SourceLabelNote,
    SourceLabelRequest,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgConnection, Postgres, Row, Transaction};
use time::OffsetDateTime;

/// Whether the review agents may try the ListingSource `s` again: its qualification is a review
/// agent's `NEEDS_HUMAN` whose retry time, which the [park schedule](ParkSchedule) set when it was
/// decided, has come. Binds nothing.
pub(crate) const AGENT_RETRY_DUE: &str = "COALESCE(s.qualification_retry_at <= NOW(), FALSE)";

/// How many `NEEDS_HUMAN` qualifications the review agents decided for the ListingSource `s` since
/// a human last decided or cleared its qualification. Binds nothing.
const AGENT_NEEDS_HUMAN_SINCE_HUMAN: &str = "(SELECT COUNT(*) \
     FROM listing_source_qualification_events e \
     WHERE e.listing_source_id = s.listing_source_id \
       AND e.qualification = 'NEEDS_HUMAN' AND e.actor IN ('AGENT_AUTHOR', 'AGENT_REVIEWER') \
       AND e.occurred_at > COALESCE( \
           (SELECT MAX(h.occurred_at) FROM listing_source_qualification_events h \
            WHERE h.listing_source_id = s.listing_source_id AND h.actor = 'HUMAN'), \
           '-infinity'::timestamptz))";

/// Whether the agents' next try of the ListingSource `s` is their final round: they decided
/// `NEEDS_HUMAN` at least `$2` times ([`AGENT_NEEDS_HUMAN_BEFORE_FINAL_ROUND`]) since a human last
/// decided or cleared its qualification. Binds `$2`.
pub(crate) fn agent_final_round_due() -> String {
    format!("({AGENT_NEEDS_HUMAN_SINCE_HUMAN} >= $2)")
}

/// What a decision reads of its source under the source's row lock.
struct SourceUnderQualification {
    in_crawl_scope: bool,
    qualification: Option<String>,
    /// Whether the agents may try the source again after their own `NEEDS_HUMAN`.
    agent_retry_due: bool,
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
            check_agent_decision_rate(
                &mut transaction,
                self.agent_decisions.limits().max_decisions_per_hour,
            )
            .await?;
        }
        let (label_effect, label) = match (decision.label(), &source.label) {
            (Some(label), _) => {
                let request = SourceLabelRequest::new(label, Some(decision.note().clone()))?;
                store_label(
                    &mut transaction,
                    listing_source_id,
                    actor,
                    &request,
                    self.agent_decisions.park_schedule(),
                )
                .await?;
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
        let stored = store_qualification(
            &mut transaction,
            listing_source_id,
            actor,
            decision,
            self.agent_decisions.park_schedule(),
        )
        .await?;
        if decision.qualification() != Qualification::Qualified {
            self.withhold_crawl(&mut transaction, listing_source_id)
                .await?;
        }
        transaction.commit().await?;
        log_committed_source(
            &self.pool,
            listing_source_id,
            ReviewLogEntry::source_qualified(actor, decision.qualification(), decision.label())
                .with_retry_at(stored.retry_at),
        )
        .await;
        match (label_effect, decision.label()) {
            (QualificationLabelEffect::Labelled, Some(label)) => {
                log_committed_source(
                    &self.pool,
                    listing_source_id,
                    ReviewLogEntry::source_labelled(actor, label, true),
                )
                .await;
            }
            (QualificationLabelEffect::ClearedLabel, _) => {
                log_committed_source(
                    &self.pool,
                    listing_source_id,
                    ReviewLogEntry::source_label_cleared(actor),
                )
                .await;
            }
            _ => {}
        }
        Ok(QualificationOutcome {
            applied: AppliedQualification {
                qualification: decision.qualification(),
                note: decision.note().as_str().to_owned(),
                qualified_at: stored.qualified_at,
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
        clear_stored_qualification(&mut transaction, listing_source_id, actor).await?;
        self.withhold_crawl(&mut transaction, listing_source_id)
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

impl CrawlerReviewRepository {
    /// Stops crawling the ListingSource in the transaction that leaves it without a qualification
    /// that admits it, where only qualified sources are crawled, instead of at the next scope
    /// refresh: until then a source the agents could not decide, or one a human cleared, would
    /// still be crawled and its reviews worked. Only the scope refresh enables a source again.
    pub(super) async fn withhold_crawl(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), ReviewRepositoryError> {
        if !self.agent_decisions.crawls_only_qualified_sources() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE listing_sources SET crawl_enabled = FALSE, updated = NOW() \
             WHERE listing_source_id = $1 AND crawl_enabled",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }
}

/// Refuses a review agent's decision on a source that is not its to decide.
fn check_agent_may_decide(
    listing_source_id: &ListingSourceId,
    source: &SourceUnderQualification,
) -> Result<(), ReviewRepositoryError> {
    if source.qualification.is_some() && !source.agent_retry_due {
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
    let row = sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT s.in_crawl_scope, s.qualification, s.source_label, s.source_labelled_by, \
                s.qualification_backfill, {AGENT_RETRY_DUE} AS agent_retry_due \
         FROM listing_sources s WHERE s.listing_source_id = $1 FOR UPDATE"
    )))
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
        agent_retry_due: row.try_get("agent_retry_due")?,
        label,
        backfill: row.try_get("qualification_backfill")?,
    })
}

/// A qualification as [`store_qualification`] stored it: when, and for a review agent's
/// `NEEDS_HUMAN` when the agents try the source again.
struct StoredQualification {
    qualified_at: OffsetDateTime,
    retry_at: Option<OffsetDateTime>,
}

/// Stores `actor`'s qualification `decision` on the ListingSource and appends it to the source's
/// qualification history, answering when. A review agent's `NEEDS_HUMAN` is tried again as `park`
/// schedules it; every other decision stands until it is changed. The first decision ends the
/// source's part in the one-off backfill: should its qualification be cleared, it waits uncrawled
/// like a new source, and no agent replaces a label a human set on it.
async fn store_qualification(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    decision: &QualificationDecision,
    park: ParkSchedule,
) -> Result<StoredQualification, ReviewRepositoryError> {
    let retry_after = if actor.is_agent() && decision.qualification() == Qualification::NeedsHuman {
        let needs_human = agent_needs_human_since_human(transaction, listing_source_id).await?;
        Some(
            park.qualification_retry_after(needs_human.saturating_add(1))
                .as_seconds_f64(),
        )
    } else {
        None
    };
    let (qualified_at, retry_at) = sqlx::query_as::<_, (OffsetDateTime, Option<OffsetDateTime>)>(
        "UPDATE listing_sources \
         SET qualification = $2, qualification_note = $3, qualified_at = NOW(), \
             qualified_by = $4, qualification_backfill = FALSE, \
             qualification_retry_at = NOW() + make_interval(secs => $5::float8), updated = NOW() \
         WHERE listing_source_id = $1 \
         RETURNING qualified_at, qualification_retry_at",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(decision.qualification().as_str())
    .bind(decision.note().as_str())
    .bind(actor.as_str())
    .bind(retry_after)
    .fetch_one(&mut **transaction)
    .await?;
    append_qualification_event(
        transaction,
        listing_source_id,
        Some(decision.qualification()),
        decision.label(),
        Some(decision.note()),
        actor,
    )
    .await?;
    Ok(StoredQualification {
        qualified_at,
        retry_at,
    })
}

/// How many `NEEDS_HUMAN` qualifications the review agents decided for the ListingSource since a
/// human last decided or cleared it, before the one being stored.
async fn agent_needs_human_since_human(
    connection: &mut PgConnection,
    listing_source_id: &ListingSourceId,
) -> Result<u32, sqlx::Error> {
    let count = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT {AGENT_NEEDS_HUMAN_SINCE_HUMAN} FROM listing_sources s \
         WHERE s.listing_source_id = $1"
    )))
    .bind(uuid::Uuid::from(*listing_source_id))
    .fetch_one(connection)
    .await?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Clears the ListingSource's qualification and its retry and appends the clear to its
/// qualification history, so the qualification queue offers the source again.
pub(super) async fn clear_stored_qualification(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE listing_sources \
         SET qualification = NULL, qualification_note = NULL, qualified_at = NULL, \
             qualified_by = NULL, qualification_retry_at = NULL, updated = NOW() \
         WHERE listing_source_id = $1",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .execute(&mut **transaction)
    .await?;
    append_qualification_event(transaction, listing_source_id, None, None, None, actor).await
}

/// Qualifies the ListingSource whose label `actor` just cleared, so a cleared source is crawled
/// again without waiting for a qualification; the note names the cleared label.
pub(super) async fn qualify_after_label_clear(
    transaction: &mut Transaction<'_, Postgres>,
    listing_source_id: &ListingSourceId,
    actor: ReviewActor,
    cleared: SourceLabel,
    park: ParkSchedule,
) -> Result<(), ReviewRepositoryError> {
    let note = SourceLabelNote::try_from(format!(
        "Qualified by clearing the label {}.",
        cleared.as_str()
    ))?;
    let decision = QualificationDecision::qualified(note);
    store_qualification(transaction, listing_source_id, actor, &decision, park).await?;
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
