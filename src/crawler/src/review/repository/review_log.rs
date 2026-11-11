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
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tracing::{Level, level_enabled, warn};
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
    /// Why a review opened, or why an agent handed it to a human
    /// (`HandoverReason::log_name`).
    reason: Option<String>,
    /// How a review was decided: `approved`, `rejected`, `needs_repair` or `handed_to_human`.
    decision: Option<&'static str>,
    /// What the author named as the obstacle of a hand-over (`HandoverCategory::log_name`).
    handover_category: Option<&'static str>,
    /// What changed on a pending review's candidate.
    change: Option<&'static str>,
    /// The URLs the change retired, reclassified or recorded.
    affected_urls: Option<usize>,
    /// Whether an approval accepted a candidate that does not cover every review page.
    incomplete_coverage: Option<bool>,
    /// How many URLs the sample labels set mark as products and as no products.
    product_labels: Option<usize>,
    not_product_labels: Option<usize>,
    /// What a mass-removal confirmation did: held removals it made due at once, how long it
    /// lasts and when it ends.
    rearmed_held_urls: Option<u64>,
    hours: Option<u32>,
    confirmed_until: Option<String>,
}

impl ReviewLogEntry {
    fn new(event: &'static str, message: &'static str, actor: Option<ReviewActor>) -> Self {
        Self {
            event,
            message,
            actor,
            reason: None,
            decision: None,
            handover_category: None,
            change: None,
            affected_urls: None,
            incomplete_coverage: None,
            product_labels: None,
            not_product_labels: None,
            rearmed_held_urls: None,
            hours: None,
            confirmed_until: None,
        }
    }

    pub(super) fn opened(reason: &str) -> Self {
        Self {
            reason: Some(reason.to_owned()),
            ..Self::new(REVIEW_OPENED_EVENT, "Review opened", None)
        }
    }

    pub(super) fn mass_removal_confirmed(
        actor: ReviewActor,
        hours: u32,
        confirmed_until: OffsetDateTime,
        rearmed_held_urls: u64,
    ) -> Self {
        Self {
            hours: Some(hours),
            confirmed_until: confirmed_until.format(&Rfc3339).ok(),
            rearmed_held_urls: Some(rearmed_held_urls),
            ..Self::new(
                MASS_REMOVAL_CONFIRMED_EVENT,
                "Mass removal confirmed; the held removals are due at once",
                Some(actor),
            )
        }
    }

    /// A review that was created already decided: its decision is its only event, which the
    /// crawler itself took.
    pub(super) fn born_decided(status: &str) -> Option<Self> {
        let decision = match status {
            "APPROVED" => "approved",
            "REJECTED" => "rejected",
            _ => return None,
        };
        Some(Self {
            decision: Some(decision),
            ..Self::new(
                REVIEW_DECIDED_EVENT,
                "Review created already decided",
                Some(ReviewActor::System),
            )
        })
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
            ReviewMutation::HandedToHuman { cause, .. } => vec![Self {
                reason: Some(cause.reason.log_name().to_owned()),
                handover_category: Some(cause.category.log_name()),
                ..decided("handed_to_human", "Review agent handed a review to a human")
            }],
            ReviewMutation::CandidateEdited { .. } => {
                vec![changed("candidate_edited", "Candidate of a review edited")]
            }
            ReviewMutation::SchemaFieldEdited { .. } => {
                vec![changed("schema_edited", "Schema field of a review edited")]
            }
            ReviewMutation::SampleLabelsEdited { sample_labels } => vec![Self {
                product_labels: Some(sample_labels.product),
                not_product_labels: Some(sample_labels.not_product),
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

/// The ListingSource a review log event is about, as far as it could be read.
#[derive(Debug, Default, Clone)]
struct LoggedSource {
    listing_source_id: Option<ListingSourceId>,
    listing_source_name: Option<String>,
    domain: Option<String>,
    artifact_type: Option<String>,
}

/// An event read for its source and waiting for the mutation to commit: the log of a change that
/// was rolled back would be false.
#[must_use = "emit the event once the mutation committed"]
pub(super) struct PreparedReviewLog {
    entry: ReviewLogEntry,
    review_id: Option<CrawlerReviewId>,
    source: LoggedSource,
}

/// Reads the source of the review `review_id`. A source that cannot be read never fails the
/// mutation the event is about: it is warned about once and the event logs without it.
async fn read_review_source<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    review_id: CrawlerReviewId,
) -> Result<Option<LoggedSource>, sqlx::Error> {
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
        return Ok(None);
    };
    Ok(Some(LoggedSource {
        listing_source_id: Some(
            ListingSourceId::try_from(row.try_get::<Uuid, _>("listing_source_id")?)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        ),
        listing_source_name: row.try_get("listing_source_name")?,
        domain: row.try_get("crawl_host")?,
        artifact_type: Some(row.try_get("artifact_type")?),
    }))
}

fn warn_source_unreadable(review_id: Option<CrawlerReviewId>, error: &sqlx::Error) {
    warn!(
        review_id = review_id.map(|id| id.to_string()),
        error = %error,
        "Cannot read the ListingSource of a review event; the event logs without its name and domain"
    );
}

fn prepare_entries(
    entries: Vec<ReviewLogEntry>,
    review_id: CrawlerReviewId,
    source: &LoggedSource,
) -> Vec<PreparedReviewLog> {
    entries
        .into_iter()
        .map(|entry| PreparedReviewLog {
            entry,
            review_id: Some(review_id),
            source: source.clone(),
        })
        .collect()
}

/// Reads the source of the review `review_id` inside the mutation's transaction, for `entries`
/// that are logged once it committed, skipping the read when every event's level is off. The read
/// runs in a savepoint, so a failed read leaves the mutation's transaction usable.
pub(super) async fn prepare_review_logs(
    connection: &mut sqlx::PgConnection,
    review_id: CrawlerReviewId,
    entries: Vec<ReviewLogEntry>,
) -> Vec<PreparedReviewLog> {
    let entries: Vec<_> = entries
        .into_iter()
        .filter(ReviewLogEntry::enabled)
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let read = async {
        let mut savepoint = sqlx::Connection::begin(&mut *connection).await?;
        match read_review_source(&mut *savepoint, review_id).await {
            Ok(source) => {
                savepoint.commit().await?;
                Ok(source)
            }
            Err(error) => {
                // The savepoint is the failed read's only transaction; a rollback that fails too
                // leaves nothing further to report than the read's own error.
                let _ = savepoint.rollback().await;
                Err(error)
            }
        }
    };
    match read.await {
        Ok(Some(source)) => prepare_entries(entries, review_id, &source),
        Ok(None) => Vec::new(),
        Err(error) => {
            warn_source_unreadable(Some(review_id), &error);
            prepare_entries(entries, review_id, &LoggedSource::default())
        }
    }
}

/// Logs `entries` of the committed review `review_id`, reading its source from `pool`.
pub(super) async fn log_committed_review(
    pool: &sqlx::PgPool,
    review_id: CrawlerReviewId,
    entries: Vec<ReviewLogEntry>,
) {
    let entries: Vec<_> = entries
        .into_iter()
        .filter(ReviewLogEntry::enabled)
        .collect();
    if entries.is_empty() {
        return;
    }
    let source = match read_review_source(pool, review_id).await {
        Ok(Some(source)) => source,
        Ok(None) => return,
        Err(error) => {
            warn_source_unreadable(Some(review_id), &error);
            LoggedSource::default()
        }
    };
    emit_all(prepare_entries(entries, review_id, &source));
}

/// As [`log_committed_review`] for an event about a ListingSource that no review owns.
pub(super) async fn log_committed_source(
    pool: &sqlx::PgPool,
    listing_source_id: &ListingSourceId,
    entry: ReviewLogEntry,
) {
    if !entry.enabled() {
        return;
    }
    let row = sqlx::query(
        "SELECT listing_source_name, crawl_host FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(Uuid::from(*listing_source_id))
    .fetch_optional(pool)
    .await
    .and_then(|row| match row {
        Some(row) => Ok((
            row.try_get("listing_source_name")?,
            row.try_get("crawl_host")?,
        )),
        None => Ok((None, None)),
    });
    let (listing_source_name, domain) = row.unwrap_or_else(|error| {
        warn_source_unreadable(None, &error);
        (None, None)
    });
    PreparedReviewLog {
        entry,
        review_id: None,
        source: LoggedSource {
            listing_source_id: Some(*listing_source_id),
            listing_source_name,
            domain,
            artifact_type: None,
        },
    }
    .emit();
}

impl PreparedReviewLog {
    pub(super) fn emit(self) {
        let Self {
            entry,
            review_id,
            source,
        } = self;
        let LoggedSource {
            listing_source_id,
            listing_source_name,
            domain,
            artifact_type,
        } = source;
        macro_rules! log_event {
            ($level:expr) => {
                tracing::event!(
                    $level,
                    event = entry.event,
                    listing_source_id = listing_source_id.map(|id| id.to_string()),
                    listing_source_name,
                    domain,
                    review_id = review_id.map(|id| id.to_string()),
                    artifact_type,
                    actor = entry.actor.map(ReviewActor::as_str),
                    reason = entry.reason,
                    decision = entry.decision,
                    handover_category = entry.handover_category,
                    change = entry.change,
                    affected_urls = entry.affected_urls,
                    product_labels = entry.product_labels,
                    not_product_labels = entry.not_product_labels,
                    rearmed_held_urls = entry.rearmed_held_urls,
                    confirmed_until = entry.confirmed_until,
                    hours = entry.hours,
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
    use test_api::IntegrationTestService as _;

    fn prepared(entry: ReviewLogEntry) -> PreparedReviewLog {
        PreparedReviewLog {
            entry,
            review_id: Some(CrawlerReviewId::new()),
            source: LoggedSource {
                listing_source_id: Some(ListingSourceId::new()),
                listing_source_name: Some("Antique Shop".to_owned()),
                domain: Some("antique.example".to_owned()),
                artifact_type: Some("PRODUCT_SCHEMA".to_owned()),
            },
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

    /// The cause is a short stable name Logs Insights groups by; the agent's reasoning, which may
    /// quote the shop's pages, stays in the event trail.
    #[test]
    fn should_log_the_reason_and_not_the_note_when_an_agent_hands_a_review_to_a_human() {
        let (events, _guard) = LoggedEvents::capture();
        let mutation = ReviewMutation::HandedToHuman {
            note: "the sample is behind a bot challenge",
            cause: crate::review::model::HandoverCause {
                reason: crate::review::model::HandoverReason::NeedsHuman,
                category: crate::review::model::HandoverCategory::PagesUnreadable,
            },
        };

        for entry in ReviewLogEntry::of_mutation(ReviewActor::AgentAuthor, &mutation) {
            prepared(entry).emit();
        }

        let decided = events.named(REVIEW_DECIDED_EVENT);
        assert_eq!(decided.len(), 1);
        for (field, expected) in [
            ("actor", "AGENT_AUTHOR"),
            ("decision", "handed_to_human"),
            ("reason", "needs_human"),
            ("handover_category", "pages_unreadable"),
        ] {
            assert_eq!(decided[0].field(field), Some(expected), "{field}");
        }
        assert!(
            events
                .all()
                .iter()
                .all(|logged| !format!("{logged:?}").contains("bot challenge")),
            "the note must not be logged"
        );
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
        assert_eq!(
            (entries[0].product_labels, entries[0].not_product_labels),
            (Some(3), Some(2))
        );
    }

    #[test]
    fn should_log_the_confirmation_end_the_hours_and_the_rearmed_urls_of_a_mass_removal() {
        let (events, _guard) = LoggedEvents::capture();
        let until = OffsetDateTime::UNIX_EPOCH;

        prepared(ReviewLogEntry::mass_removal_confirmed(
            ReviewActor::Human,
            24,
            until,
            7,
        ))
        .emit();

        let confirmed = events.named(MASS_REMOVAL_CONFIRMED_EVENT);
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].level, Level::INFO);
        for (field, expected) in [
            ("hours", "24"),
            ("rearmed_held_urls", "7"),
            ("confirmed_until", "1970-01-01T00:00:00Z"),
            ("actor", "HUMAN"),
        ] {
            assert_eq!(confirmed[0].field(field), Some(expected), "{field}");
        }
    }

    #[test]
    fn should_log_a_review_created_already_approved_as_the_crawler_s_decision() {
        let entry = ReviewLogEntry::born_decided("APPROVED").expect("a decided status");

        assert_eq!(
            (entry.decision, entry.actor),
            (Some("approved"), Some(ReviewActor::System))
        );
        assert!(ReviewLogEntry::born_decided("PENDING_REVIEW").is_none());
    }

    /// A log must never fail or roll back the decision it is about.
    #[test_api::aura_integration_test(services = [crate::test_support::CrawlerDatabase], isolated = true)]
    async fn should_commit_the_decision_and_log_it_without_the_source_when_the_source_cannot_be_read()
     {
        use crate::review::model::UrlPatternDecision;
        use crate::review::repository::CrawlerReviewRepository;
        let pool = test_api::get_postgres_client().await;
        let listing_source_id = ListingSourceId::new();
        sqlx::query(
            "INSERT INTO listing_sources \
             (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, created, updated) \
             VALUES ($1, 'Antique shop', 'antique-shop', TRUE, NOW(), NOW())",
        )
        .bind(listing_source_id.as_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        let repository = CrawlerReviewRepository::new(pool.clone());
        let review_id = repository
            .create_url_pattern_review(
                &listing_source_id,
                "manual_url_pattern_required",
                UrlPatternDecision::Undecided,
                &[],
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("create review: {error}"));
        sqlx::query(
            "ALTER TABLE listing_sources RENAME COLUMN listing_source_name TO renamed_name",
        )
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("break the source read: {error}"));
        let (events, _guard) = LoggedEvents::capture();

        repository
            .reject_review(review_id, ReviewActor::Human, None)
            .await
            .unwrap_or_else(|error| panic!("the decision commits: {error}"));

        let status: String =
            sqlx::query_scalar("SELECT status FROM crawler_reviews WHERE review_id = $1")
                .bind(review_id.as_uuid())
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("read status: {error}"));
        assert_eq!(status, "REJECTED");
        let decided = events.named(REVIEW_DECIDED_EVENT);
        assert_eq!(decided.len(), 1);
        assert_eq!(decided[0].field("decision"), Some("rejected"));
        assert!(decided[0].field("listing_source_name").is_none());
        assert!(
            events
                .all()
                .iter()
                .any(|logged| logged.level == Level::WARN && logged.field("error").is_some())
        );
    }
}
