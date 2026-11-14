//! The submission limits of `POST /v1/content-notices`: an identical resubmission is not stored
//! again, and one email address may store only a bounded number of notices per window. See
//! [submission limits](../../../docs/content-notices.md#submission-limits).

use crate::ports::{ContentNoticeSubmissionFingerprint, RecentSubmission};
use time::{Duration, OffsetDateTime};

/// How far back a submission counts, rolling: a notice counts while it was created less than
/// this long before the submission's `now`. Approved by the product owner.
pub const SUBMISSION_WINDOW: Duration = Duration::hours(24);

/// How many notices one email address may have stored within [`SUBMISSION_WINDOW`]. A further
/// submission is rate limited; identical resubmissions are not stored and do not count.
/// Approved by the product owner.
pub const MAX_NOTICES_PER_ADDRESS: usize = 10;

/// What the submission limits allow for one submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionDecision {
    /// Store the notice.
    Store,
    /// The same submission is already stored within the window: store nothing and answer as
    /// if it were stored.
    Duplicate,
    /// The address has used its budget for the window.
    RateLimited { retry_after: RetryAfter },
}

/// Whole seconds until a rate-limited address can store a notice again, at least one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(u64);

impl RetryAfter {
    /// Rounds up to whole seconds, so a retry after it is never too early.
    pub fn rounded_up(duration: Duration) -> Self {
        let seconds = duration.whole_seconds() + i64::from(duration.subsec_nanoseconds() > 0);
        Self(u64::try_from(seconds).unwrap_or(0).max(1))
    }

    pub const fn seconds(self) -> u64 {
        self.0
    }
}

/// The oldest creation time that can still be inside the window at `now`, exclusive.
pub fn window_start(now: OffsetDateTime) -> OffsetDateTime {
    now - SUBMISSION_WINDOW
}

/// Decides a submission with `fingerprint` from the stored submissions of the same address.
/// Submissions outside the window at `now` are ignored. The duplicate check comes first, so a
/// resubmission is never rate limited.
pub fn decide(
    now: OffsetDateTime,
    fingerprint: ContentNoticeSubmissionFingerprint,
    submissions: &[RecentSubmission],
) -> SubmissionDecision {
    let start = window_start(now);
    let mut in_window = submissions
        .iter()
        .filter(|submission| submission.created > start)
        .collect::<Vec<_>>();
    if in_window
        .iter()
        .any(|submission| submission.fingerprint == fingerprint)
    {
        return SubmissionDecision::Duplicate;
    }
    if in_window.len() < MAX_NOTICES_PER_ADDRESS {
        return SubmissionDecision::Store;
    }
    // The address is below its budget again once all but `MAX - 1` of these left the window.
    in_window.sort_unstable_by_key(|submission| submission.created);
    let freeing = in_window[in_window.len() - MAX_NOTICES_PER_ADDRESS].created;
    SubmissionDecision::RateLimited {
        retry_after: RetryAfter::rounded_up(freeing + SUBMISSION_WINDOW - now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-11-14 12:00:00 UTC);
    const SUBMITTED: ContentNoticeSubmissionFingerprint =
        ContentNoticeSubmissionFingerprint::from_sha256([1; 32]);

    fn other(index: u8) -> ContentNoticeSubmissionFingerprint {
        ContentNoticeSubmissionFingerprint::from_sha256([index.wrapping_add(100); 32])
    }

    fn submission(
        fingerprint: ContentNoticeSubmissionFingerprint,
        age: Duration,
    ) -> RecentSubmission {
        RecentSubmission {
            fingerprint,
            created: NOW - age,
        }
    }

    /// `count` distinct stored submissions, the oldest `oldest_age` ago and one minute apart.
    fn distinct(count: usize, oldest_age: Duration) -> Vec<RecentSubmission> {
        (0..count)
            .map(|index| {
                submission(
                    other(index as u8),
                    oldest_age - Duration::minutes(index as i64),
                )
            })
            .collect()
    }

    #[test]
    fn should_store_the_first_submission_of_an_address() {
        assert_eq!(SubmissionDecision::Store, decide(NOW, SUBMITTED, &[]));
    }

    #[test]
    fn should_treat_identical_submission_inside_the_window_as_duplicate() {
        let just_inside = SUBMISSION_WINDOW - Duration::nanoseconds(1);

        assert_eq!(
            SubmissionDecision::Duplicate,
            decide(NOW, SUBMITTED, &[submission(SUBMITTED, just_inside)])
        );
    }

    #[test]
    fn should_store_identical_submission_again_once_it_left_the_window() {
        assert_eq!(
            SubmissionDecision::Store,
            decide(NOW, SUBMITTED, &[submission(SUBMITTED, SUBMISSION_WINDOW)])
        );
    }

    #[test]
    fn should_accept_the_tenth_notice_and_reject_the_eleventh() {
        let nine = distinct(MAX_NOTICES_PER_ADDRESS - 1, Duration::hours(1));
        let ten = distinct(MAX_NOTICES_PER_ADDRESS, Duration::hours(1));

        assert_eq!(SubmissionDecision::Store, decide(NOW, SUBMITTED, &nine));
        assert!(matches!(
            decide(NOW, SUBMITTED, &ten),
            SubmissionDecision::RateLimited { .. }
        ));
    }

    #[test]
    fn should_count_a_different_fingerprint_from_the_same_address() {
        let mut submissions = distinct(MAX_NOTICES_PER_ADDRESS - 1, Duration::hours(1));
        assert_eq!(
            SubmissionDecision::Store,
            decide(NOW, SUBMITTED, &submissions)
        );

        submissions.push(submission(other(200), Duration::minutes(1)));

        assert!(matches!(
            decide(NOW, SUBMITTED, &submissions),
            SubmissionDecision::RateLimited { .. }
        ));
    }

    /// Duplicates are never stored, so stored submissions are distinct; a resubmission of one of
    /// them must still be answered as a duplicate when the budget is used up.
    #[test]
    fn should_answer_a_duplicate_even_when_the_budget_is_used_up() {
        let mut submissions = distinct(MAX_NOTICES_PER_ADDRESS - 1, Duration::hours(1));
        submissions.push(submission(SUBMITTED, Duration::minutes(1)));

        assert_eq!(
            SubmissionDecision::Duplicate,
            decide(NOW, SUBMITTED, &submissions)
        );
    }

    #[test]
    fn should_not_count_submissions_outside_the_window() {
        let mut submissions = distinct(MAX_NOTICES_PER_ADDRESS - 1, Duration::hours(1));
        submissions.push(submission(other(200), SUBMISSION_WINDOW));

        assert_eq!(
            SubmissionDecision::Store,
            decide(NOW, SUBMITTED, &submissions)
        );
    }

    #[test]
    fn should_retry_after_the_oldest_notice_in_the_window_leaves_it() {
        let oldest_age = Duration::hours(23) + Duration::seconds(30);
        let mut submissions = distinct(MAX_NOTICES_PER_ADDRESS, oldest_age);
        submissions.reverse();

        assert_eq!(
            SubmissionDecision::RateLimited {
                retry_after: RetryAfter(Duration::minutes(59).whole_seconds() as u64 + 30),
            },
            decide(NOW, SUBMITTED, &submissions)
        );
    }

    #[test]
    fn should_round_retry_after_up_to_whole_seconds_and_at_least_one() {
        assert_eq!(
            2,
            RetryAfter::rounded_up(Duration::milliseconds(1_001)).seconds()
        );
        assert_eq!(1, RetryAfter::rounded_up(Duration::seconds(1)).seconds());
        assert_eq!(
            1,
            RetryAfter::rounded_up(Duration::nanoseconds(1)).seconds()
        );
        assert_eq!(1, RetryAfter::rounded_up(Duration::ZERO).seconds());
        assert_eq!(1, RetryAfter::rounded_up(Duration::seconds(-5)).seconds());
    }

    /// Notices stored before the budget applied, or before it was lowered, can exceed it; the
    /// address may submit again once enough of them left the window to fall below it.
    #[test]
    fn should_retry_after_enough_notices_left_the_window_when_over_budget() {
        let submissions = distinct(MAX_NOTICES_PER_ADDRESS + 2, Duration::hours(23));

        assert_eq!(
            SubmissionDecision::RateLimited {
                retry_after: RetryAfter(Duration::hours(1).whole_seconds() as u64 + 2 * 60),
            },
            decide(NOW, SUBMITTED, &submissions)
        );
    }
}
