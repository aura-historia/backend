//! The INFO event of every review state change, `crawler.review.*`, with the source fields
//! `listing_source_id`, `listing_source_name` and `domain` that every outcome event carries. The
//! review row knows its ListingSource, so one read per event names it; the event trail
//! ([`super::review_events`]) keeps the details, and a review agent makes many small candidate
//! changes per review, which log at DEBUG.

use super::review_events::ReviewMutation;
use crate::CrawlerReviewId;
use crate::review::model::ReviewActor;
use listing_source_core::ListingSourceId;
use sqlx::Row;
use tracing::{Level, level_enabled};
use uuid::Uuid;

pub(super) const REVIEW_OPENED_EVENT: &str = "crawler.review.opened";
pub(super) const REVIEW_DECIDED_EVENT: &str = "crawler.review.decided";
pub(super) const REVIEW_CANDIDATE_CHANGED_EVENT: &str = "crawler.review.candidate_changed";
pub(super) const REVIEW_RESTORED_EVENT: &str = "crawler.review.restored";
pub(super) const REVIEW_REJECTION_REVOKED_EVENT: &str = "crawler.review.rejection_revoked";
pub(super) const REVIEW_URLS_RETIRED_NOT_PRODUCT_EVENT: &str =
    "crawler.review.urls_retired_not_product";
pub(super) const MASS_REMOVAL_CONFIRMED_EVENT: &str = "crawler.source.mass_removal_confirmed";
pub(super) const MASS_REMOVAL_REVOKED_EVENT: &str = "crawler.source.mass_removal_revoked";

/// What one review state change logs besides the source: which event, who acted, and the one
/// detail that names the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewLogEntry {
    event: &'static str,
    message: &'static str,
    actor: Option<ReviewActor>,
    /// Why a review opened.
    reason: Option<String>,
    /// How a review was decided: `approved`, `rejected`, `needs_repair` or `handed_to_human`.
    decision: Option<&'static str>,
    /// What changed on a pending review's candidate.
    change: Option<&'static str>,
    /// The URLs the change retired, reclassified or recorded.
    affected_urls: Option<usize>,
    /// Whether an approval accepted a candidate that does not cover every review page.
    incomplete_coverage: Option<bool>,
}

impl ReviewLogEntry {
    fn new(event: &'static str, message: &'static str, actor: Option<ReviewActor>) -> Self {
        Self {
            event,
            message,
            actor,
            reason: None,
            decision: None,
            change: None,
            affected_urls: None,
            incomplete_coverage: None,
        }
    }

    pub(super) fn opened(reason: &str) -> Self {
        Self {
            reason: Some(reason.to_owned()),
            ..Self::new(REVIEW_OPENED_EVENT, "Review opened", None)
        }
    }

    pub(super) fn mass_removal_confirmed(actor: ReviewActor) -> Self {
        Self::new(
            MASS_REMOVAL_CONFIRMED_EVENT,
            "Mass removal confirmed; the held removals are due at once",
            Some(actor),
        )
    }

    pub(super) fn mass_removal_revoked(actor: ReviewActor) -> Self {
        Self::new(
            MASS_REMOVAL_REVOKED_EVENT,
            "Mass-removal confirmation revoked; the guard holds removals again",
            Some(actor),
        )
    }

    pub(super) fn with_affected_urls(mut self, urls: usize) -> Self {
        self.affected_urls = Some(urls);
        self
    }

    /// The events `actor`'s accepted `mutation` logs: one for every mutation, and a second for
    /// an approval that retired URLs of pages marked as no product page.
    pub(super) fn of_mutation(actor: ReviewActor, mutation: &ReviewMutation<'_>) -> Vec<Self> {
        let decided = |decision, message| Self {
            decision: Some(decision),
            ..Self::new(REVIEW_DECIDED_EVENT, message, Some(actor))
        };
        let changed = |change, message| Self {
            change: Some(change),
            ..Self::new(REVIEW_CANDIDATE_CHANGED_EVENT, message, Some(actor))
        };
        match mutation {
            ReviewMutation::ProductSchemasApproved {
                coverage,
                retired_not_product_urls,
                ..
            } => {
                let mut events = vec![Self {
                    incomplete_coverage: Some(!coverage.is_complete()),
                    ..decided("approved", "Review approved")
                }];
                if !retired_not_product_urls.is_empty() {
                    events.push(
                        Self::new(
                            REVIEW_URLS_RETIRED_NOT_PRODUCT_EVENT,
                            "Retired the URLs of the pages marked as no product page",
                            Some(actor),
                        )
                        .with_affected_urls(retired_not_product_urls.len()),
                    );
                }
                events
            }
            ReviewMutation::UrlPatternApproved { .. } => {
                vec![decided("approved", "Review approved")]
            }
            ReviewMutation::Rejected { .. } => vec![decided("rejected", "Review rejected")],
            ReviewMutation::RepairRequested { .. } => vec![decided(
                "needs_repair",
                "Reviewer asked for a review to be repaired",
            )],
            ReviewMutation::HandedToHuman { .. } => vec![decided(
                "handed_to_human",
                "Review agent handed a review to a human",
            )],
            ReviewMutation::CandidateEdited { .. } => {
                vec![changed("candidate_edited", "Candidate of a review edited")]
            }
            ReviewMutation::SchemaFieldEdited { .. } => {
                vec![changed("schema_edited", "Schema field of a review edited")]
            }
            ReviewMutation::SampleLabelsEdited { sample_labels } => vec![Self {
                affected_urls: Some(sample_labels.product + sample_labels.not_product),
                ..changed("labels_set", "Sample labels of a URL-pattern review set")
            }],
            ReviewMutation::PageAdded { .. } => {
                vec![changed("page_added", "Page added to a review")]
            }
            ReviewMutation::PageRemoved { .. } => {
                vec![changed("page_removed", "Page removed from a review")]
            }
            ReviewMutation::PageMarkedNotProduct { .. } => vec![changed(
                "page_marked_not_product",
                "Review page marked as no product page",
            )],
            ReviewMutation::PageNotProductCleared { .. } => vec![changed(
                "mark_cleared",
                "Mark of a review page as no product page cleared",
            )],
            ReviewMutation::NoSoldPageRecorded { checked_urls, .. } => vec![Self {
                affected_urls: Some(checked_urls.len()),
                ..changed(
                    "no_sold_page_recorded",
                    "No sold or unavailable page recorded for a review",
                )
            }],
            ReviewMutation::ProductSchemasRestored { .. }
            | ReviewMutation::UrlPatternRestored { .. } => vec![Self::new(
                REVIEW_RESTORED_EVENT,
                "Restored what a review's approval replaced",
                Some(actor),
            )],
            ReviewMutation::RejectionRevoked {
                reclassified_urls, ..
            } => vec![
                Self::new(
                    REVIEW_REJECTION_REVOKED_EVENT,
                    "Revoked the rejection of a review",
                    Some(actor),
                )
                .with_affected_urls(reclassified_urls.len()),
            ],
        }
    }

    /// A review agent's candidate changes repeat many times per review, and the event trail keeps
    /// them: only a human's change is an INFO outcome.
    fn level(&self) -> Level {
        let by_agent = self.actor.is_some_and(ReviewActor::is_agent);
        if self.event == REVIEW_CANDIDATE_CHANGED_EVENT && by_agent {
            Level::DEBUG
        } else {
            Level::INFO
        }
    }

    fn enabled(&self) -> bool {
        match self.level() {
            Level::DEBUG => level_enabled!(Level::DEBUG),
            _ => level_enabled!(Level::INFO),
        }
    }
}

/// An event read for its source and waiting for the mutation to commit: the log of a change that
/// was rolled back would be false.
#[must_use = "emit the event once the mutation committed"]
pub(super) struct PreparedReviewLog {
    entry: ReviewLogEntry,
    review_id: Option<CrawlerReviewId>,
    listing_source_id: ListingSourceId,
    listing_source_name: Option<String>,
    domain: Option<String>,
    artifact_type: Option<String>,
}

/// Reads the source of the review `review_id` for `entries`, skipping the read of every event
/// whose level is off.
pub(super) async fn prepare_review_logs<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
    entries: Vec<ReviewLogEntry>,
) -> Result<Vec<PreparedReviewLog>, sqlx::Error> {
    let entries: Vec<_> = entries
        .into_iter()
        .filter(ReviewLogEntry::enabled)
        .collect();
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let Some(row) = sqlx::query(
        "SELECT r.listing_source_id, r.artifact_type, s.listing_source_name, s.crawl_host \
         FROM crawler_reviews r \
         LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id \
         WHERE r.review_id = $1",
    )
    .bind(review_id.as_uuid())
    .fetch_optional(executor)
    .await?
    else {
        return Ok(Vec::new());
    };
    let (listing_source_id, artifact_type, name, domain) = (
        ListingSourceId::try_from(row.try_get::<Uuid, _>("listing_source_id")?)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        row.try_get::<String, _>("artifact_type")?,
        row.try_get::<Option<String>, _>("listing_source_name")?,
        row.try_get::<Option<String>, _>("crawl_host")?,
    );
    Ok(entries
        .into_iter()
        .map(|entry| PreparedReviewLog {
            entry,
            review_id: Some(review_id),
            listing_source_id,
            listing_source_name: name.clone(),
            domain: domain.clone(),
            artifact_type: Some(artifact_type.clone()),
        })
        .collect())
}

/// As [`prepare_review_logs`] for an event about a ListingSource that no review owns.
pub(super) async fn prepare_source_log<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    listing_source_id: &ListingSourceId,
    entry: ReviewLogEntry,
) -> Result<Option<PreparedReviewLog>, sqlx::Error> {
    if !entry.enabled() {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT listing_source_name, crawl_host FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(Uuid::from(*listing_source_id))
    .fetch_optional(executor)
    .await?;
    let (name, domain) = match row {
        Some(row) => (
            row.try_get::<Option<String>, _>("listing_source_name")?,
            row.try_get::<Option<String>, _>("crawl_host")?,
        ),
        None => (None, None),
    };
    Ok(Some(PreparedReviewLog {
        entry,
        review_id: None,
        listing_source_id: *listing_source_id,
        listing_source_name: name,
        domain,
        artifact_type: None,
    }))
}

impl PreparedReviewLog {
    pub(super) fn emit(self) {
        let Self {
            entry,
            review_id,
            listing_source_id,
            listing_source_name,
            domain,
            artifact_type,
        } = self;
        macro_rules! log_event {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    event = entry.event,
                    listing_source_id = %listing_source_id,
                    listing_source_name,
                    domain,
                    review_id = review_id.map(|id| id.to_string()),
                    artifact_type,
                    actor = entry.actor.map(ReviewActor::as_str),
                    reason = entry.reason,
                    decision = entry.decision,
                    change = entry.change,
                    affected_urls = entry.affected_urls,
                    incomplete_coverage = entry.incomplete_coverage,
                    "{}",
                    entry.message
                )
            };
        }
        match entry.level() {
            Level::DEBUG => log_event!(Level::DEBUG),
            _ => log_event!(Level::INFO),
        }
    }
}

/// Emits every prepared event of a mutation that committed.
pub(super) fn emit_all(prepared: Vec<PreparedReviewLog>) {
    for log in prepared {
        log.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::SampleLabelCounts;
    use crate::test_support::logged_events::LoggedEvents;

    fn prepared(entry: ReviewLogEntry) -> PreparedReviewLog {
        PreparedReviewLog {
            entry,
            review_id: Some(CrawlerReviewId::new()),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: Some("Antique Shop".to_owned()),
            domain: Some("antique.example".to_owned()),
            artifact_type: Some("PRODUCT_SCHEMA".to_owned()),
        }
    }

    #[test]
    fn should_log_the_decision_with_the_source_and_the_actor_when_a_review_is_decided() {
        let (events, _guard) = LoggedEvents::capture();
        let mutation = ReviewMutation::Rejected { notes: None };

        for entry in ReviewLogEntry::of_mutation(ReviewActor::AgentReviewer, &mutation) {
            prepared(entry).emit();
        }

        let decided = events.named(REVIEW_DECIDED_EVENT);
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].level, Level::INFO);
        for (field, expected) in [
            ("listing_source_name", "Antique Shop"),
            ("domain", "antique.example"),
            ("artifact_type", "PRODUCT_SCHEMA"),
            ("actor", "AGENT_REVIEWER"),
            ("decision", "rejected"),
        ] {
            assert_eq!(decided[0].field(field), Some(expected), "{field}");
        }
        assert!(decided[0].field("listing_source_id").is_some());
        assert!(decided[0].field("review_id").is_some());
    }

    #[test]
    fn should_log_candidate_changes_at_debug_for_agents_and_at_info_for_humans() {
        let (events, _guard) = LoggedEvents::capture();
        let mutation = ReviewMutation::PageAdded {
            url: "https://antique.example/item/1",
            role: "REVIEWER_ADDED",
        };

        for actor in [
            ReviewActor::AgentAuthor,
            ReviewActor::AgentReviewer,
            ReviewActor::Human,
        ] {
            for entry in ReviewLogEntry::of_mutation(actor, &mutation) {
                prepared(entry).emit();
            }
        }

        let levels: Vec<_> = events
            .named(REVIEW_CANDIDATE_CHANGED_EVENT)
            .iter()
            .map(|logged| (logged.field("actor").map(str::to_owned), logged.level))
            .collect();
        assert_eq!(
            levels,
            [
                (Some("AGENT_AUTHOR".to_owned()), Level::DEBUG),
                (Some("AGENT_REVIEWER".to_owned()), Level::DEBUG),
                (Some("HUMAN".to_owned()), Level::INFO),
            ]
        );
    }

    #[test]
    fn should_name_the_label_counts_when_a_human_sets_sample_labels() {
        let mutation = ReviewMutation::SampleLabelsEdited {
            sample_labels: SampleLabelCounts {
                product: 3,
                not_product: 2,
            },
        };

        let entries = ReviewLogEntry::of_mutation(ReviewActor::Human, &mutation);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].change, Some("labels_set"));
        assert_eq!(entries[0].affected_urls, Some(5));
    }
}
