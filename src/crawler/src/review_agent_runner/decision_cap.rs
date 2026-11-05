//! The console's cap on the review agents' decisions as the runner sees it: when a reviewer's
//! report shows that the console refused (or would refuse) a verdict with `AGENT_DECISION_LIMIT`,
//! no further reviewer process can decide until the rolling hour has room again, so the runner
//! pauses its reviewers until then.
//!
//! The report carries the refusal only as its kind and free text (`server_answer`) and no retry
//! time, and the console answers the retry time to the reviewer, not to the runner. The runner therefore
//! computes the time the console computes: the agents' approvals and rejections of the last hour
//! come from the event trail it already reads, the cap from the console's limits, and the wait
//! ends when enough of the oldest decisions leave the hour (`agent_decision_wait` in
//! `review/repository/verdicts.rs`). The operator contract is in `docs/crawler/operations.md`
//! (review agent runner).

use super::digest::timestamp;
use super::queue::ReviewEventEntry;
use super::reports::{ReviewerReport, ServerOutcome};
use super::state::ReviewerPause;
use crate::review::agent_decisions::AgentLimits;
use crate::review::model::{ReviewActor, ReviewEventAction};
use time::{Duration, OffsetDateTime};
use tracing::info;

/// The console's reason for refusing a verdict above the cap.
pub const DECISION_LIMIT_REASON: &str = "AGENT_DECISION_LIMIT";

/// The window the console counts the agents' decisions in.
pub const DECISION_WINDOW: Duration = Duration::hours(1);

/// How much further back than the window the runner reads the trail, so a runner clock ahead of
/// the console's database misses none of the decisions the console still counts.
pub const CLOCK_MARGIN: Duration = Duration::minutes(5);

/// The console's error kind of a refusal with status 429.
const RATE_LIMITED: &str = "RATE_LIMITED";

/// The reason of the console's other 429, for its live jobs, which no verdict answers.
const LIVE_JOBS_BUSY: &str = "LIVE_JOBS_BUSY";

/// Whether the reviewer's report says the console refused its verdict, or it saw the refusal
/// coming, because of the agents' decision cap: the report names the cap's reason, or the console
/// refused the verdict as `RATE_LIMITED`, which an approval or rejection answers only at the cap.
/// A refusal of another kind, such as a `CONFLICT`, or `RATE_LIMITED` with `LIVE_JOBS_BUSY` from a
/// live job, is not the cap. The event trail decides whether, and how long, the reviewers pause.
pub fn report_shows_decision_cap(report: &ReviewerReport) -> bool {
    let answer = &report.server_answer;
    let names = |reason: &str| answer.detail.contains(reason) || answer.kind.contains(reason);
    match answer.outcome {
        ServerOutcome::Accepted => false,
        ServerOutcome::Refused if answer.kind.trim() == RATE_LIMITED => !names(LIVE_JOBS_BUSY),
        ServerOutcome::Refused | ServerOutcome::NotSent => names(DECISION_LIMIT_REASON),
    }
}

/// When the agents' next decision is taken at the earliest: once enough of their decisions leave
/// the hour to get below `limits.max_decisions_per_hour`, as the console computes its
/// `retry_after_secs`. `None` while the cap has room at `now`. `events` is the trail since at least
/// `now - DECISION_WINDOW`, oldest first; decisions that already left the hour do not change the
/// answer, so a trail read from further back is safe against a runner clock ahead of the console.
pub fn decision_cap_resumes_at(
    events: &[ReviewEventEntry],
    limits: &AgentLimits,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    let mut decided: Vec<OffsetDateTime> = events
        .iter()
        .filter(|event| is_counted_decision(event))
        .map(|event| event.created)
        .collect();
    decided.sort_unstable();
    let cap = usize::try_from(limits.max_decisions_per_hour.get()).unwrap_or(usize::MAX);
    let last_to_leave = decided.len().checked_sub(cap)?;
    Some(decided[last_to_leave] + DECISION_WINDOW).filter(|resumes| *resumes > now)
}

/// Logs, once, that the reviewer pause `ended` is over and reviewers start again.
pub fn log_reviewer_pause_ended(ended: ReviewerPause) {
    info!(
        resumed_at = %timestamp(ended.until),
        "The reviewer pause after the agents' decision cap ended; reviewer processes start again"
    );
}

/// Whether the console's cap counts `event`: an agent's approval or rejection.
fn is_counted_decision(event: &ReviewEventEntry) -> bool {
    ReviewActor::AGENTS
        .iter()
        .any(|actor| event.actor == actor.as_str())
        && [ReviewEventAction::Approved, ReviewEventAction::Rejected]
            .iter()
            .any(|action| event.action == action.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrawlerReviewEventId;
    use crate::CrawlerReviewId;
    use crate::review_agent_runner::reports::{
        Confidence, ReviewerDecision, ServerAnswer, ServerOutcome,
    };
    use std::num::NonZeroU32;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-04 12:00 UTC);

    fn event(actor: ReviewActor, action: ReviewEventAction, minutes_ago: i64) -> ReviewEventEntry {
        ReviewEventEntry {
            event_id: CrawlerReviewEventId::new(),
            review_id: CrawlerReviewId::new(),
            artifact_type: "PRODUCT_SCHEMAS".to_owned(),
            listing_source_name: None,
            actor: actor.as_str().to_owned(),
            action: action.as_str().to_owned(),
            candidate_version: 1,
            notes: None,
            created: NOW - Duration::minutes(minutes_ago),
        }
    }

    fn limits(cap: u32) -> AgentLimits {
        AgentLimits {
            max_decisions_per_hour: NonZeroU32::new(cap).unwrap_or(NonZeroU32::MIN),
            ..AgentLimits::default()
        }
    }

    fn report(outcome: ServerOutcome, kind: &str, detail: &str) -> ReviewerReport {
        ReviewerReport {
            review_id: CrawlerReviewId::new().to_string(),
            decision: ReviewerDecision::Approve,
            confidence: Confidence::High,
            summary: String::new(),
            risks: Vec::new(),
            field_findings: Vec::new(),
            server_answer: ServerAnswer {
                outcome,
                kind: kind.to_owned(),
                failed_checks: Vec::new(),
                detail: detail.to_owned(),
            },
        }
    }

    #[test]
    fn should_recognise_the_decision_cap_in_a_refusal_and_in_a_decision_not_sent() {
        let refused = report(
            ServerOutcome::Refused,
            "RATE_LIMITED",
            "AGENT_DECISION_LIMIT",
        );
        let not_sent = report(
            ServerOutcome::NotSent,
            "",
            "RATE_LIMITED, reason AGENT_DECISION_LIMIT, retry after 2872 seconds",
        );

        let paraphrased = report(
            ServerOutcome::Refused,
            "RATE_LIMITED",
            "the agents made their decisions of the hour",
        );

        assert!(report_shows_decision_cap(&refused));
        assert!(report_shows_decision_cap(&not_sent));
        assert!(report_shows_decision_cap(&paraphrased));
    }

    #[test]
    fn should_not_take_other_refusals_or_accepted_decisions_for_the_cap() {
        let busy = report(ServerOutcome::Refused, "RATE_LIMITED", "LIVE_JOBS_BUSY");
        let busy_not_sent = report(ServerOutcome::NotSent, "RATE_LIMITED", "console busy");
        let conflict = report(ServerOutcome::Refused, "CONFLICT", "HUMAN_REQUIRED");
        let accepted = report(ServerOutcome::Accepted, "", "AGENT_DECISION_LIMIT");

        assert!(!report_shows_decision_cap(&busy));
        assert!(!report_shows_decision_cap(&busy_not_sent));
        assert!(!report_shows_decision_cap(&conflict));
        assert!(!report_shows_decision_cap(&accepted));
    }

    #[test]
    fn should_resume_when_the_oldest_decision_that_holds_the_cap_leaves_the_hour() {
        let events = [
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 50),
            event(ReviewActor::AgentReviewer, ReviewEventAction::Rejected, 20),
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 5),
        ];

        let resume = decision_cap_resumes_at(&events, &limits(2), NOW);

        // Three decisions against a cap of 2: the two oldest must leave the hour before one more
        // fits, so the wait ends when the second oldest does.
        assert_eq!(resume, Some(NOW - Duration::minutes(20) + DECISION_WINDOW));
    }

    #[test]
    fn should_answer_the_same_time_from_a_trail_read_further_back() {
        let in_the_hour = [
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 40),
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 30),
        ];
        let from_further_back = [
            event(ReviewActor::AgentReviewer, ReviewEventAction::Rejected, 63),
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 61),
            in_the_hour[0].clone(),
            in_the_hour[1].clone(),
        ];

        let expected = Some(NOW - Duration::minutes(40) + DECISION_WINDOW);
        assert_eq!(
            decision_cap_resumes_at(&in_the_hour, &limits(2), NOW),
            expected
        );
        assert_eq!(
            decision_cap_resumes_at(&from_further_back, &limits(2), NOW),
            expected
        );
        assert_eq!(
            decision_cap_resumes_at(&from_further_back[..2], &limits(2), NOW),
            None,
            "decisions that all left the hour leave room"
        );
    }

    #[test]
    fn should_count_only_agent_approvals_and_rejections_of_the_last_hour() {
        let events = [
            event(ReviewActor::AgentReviewer, ReviewEventAction::Approved, 90),
            event(ReviewActor::Human, ReviewEventAction::Approved, 10),
            event(
                ReviewActor::AgentReviewer,
                ReviewEventAction::RepairRequested,
                10,
            ),
            event(ReviewActor::AgentAuthor, ReviewEventAction::Approved, 10),
        ];

        assert_eq!(decision_cap_resumes_at(&events, &limits(2), NOW), None);
        assert_eq!(
            decision_cap_resumes_at(&events, &limits(1), NOW),
            Some(NOW - Duration::minutes(10) + DECISION_WINDOW)
        );
    }
}
