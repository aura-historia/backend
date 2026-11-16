use super::ContentNoticeSubmissionFingerprint;
use application::error::BoxError;
use content_notice_core::{content_notice_target::ContentNoticeTarget, submitter::SubmitterEmail};
use time::OffsetDateTime;

/// The stored notices a submission is limited against: the submission limits count and compare
/// notices only within one group. See `submission_policy::submission_group`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionGroup<'a> {
    /// The notices from one email address. An address is identified by its lowercased form,
    /// the key stored with every notice, so addresses that differ only in case share one group.
    Address(&'a SubmitterEmail),
    /// The notices without an email address about one target. Notices with an address about
    /// the same target are not part of it.
    TargetWithoutAddress(ContentNoticeTarget),
}

/// A stored notice as the submission limits see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentSubmission {
    pub fingerprint: ContentNoticeSubmissionFingerprint,
    pub created: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum ContentNoticeSubmissionHistoryError {
    #[error("temporary content notice submission history failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted content notice submission history")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

/// The notices stored per [`SubmissionGroup`], for the submission limits.
#[async_trait::async_trait]
pub trait ContentNoticeSubmissionHistory: Send {
    /// Serializes submissions in the group until the transaction ends, so the history read
    /// after it stays current until the notice is stored.
    async fn lock_group(
        &mut self,
        group: SubmissionGroup<'_>,
    ) -> Result<(), ContentNoticeSubmissionHistoryError>;

    /// The group's notices created after `after`.
    async fn submissions_after(
        &mut self,
        group: SubmissionGroup<'_>,
        after: OffsetDateTime,
    ) -> Result<Vec<RecentSubmission>, ContentNoticeSubmissionHistoryError>;
}

pub trait ContentNoticeSubmissionHistoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx)
    -> impl ContentNoticeSubmissionHistory + 'tx;
}

/// The time a submission is decided at, so tests can control the submission window.
pub trait ContentNoticeClock: Send + Sync {
    fn now_utc(&self) -> OffsetDateTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemContentNoticeClock;

impl ContentNoticeClock for SystemContentNoticeClock {
    fn now_utc(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}
