use super::ContentNoticeSubmissionFingerprint;
use application::error::BoxError;
use content_notice_core::submitter::SubmitterEmail;
use time::OffsetDateTime;

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

/// The notices stored per email address, for the submission limits. An address is identified
/// by its lowercased form, the key stored with every notice, so addresses that differ only in
/// case share one history.
#[async_trait::async_trait]
pub trait ContentNoticeSubmissionHistory: Send {
    /// Serializes submissions from the address until the transaction ends, so the history read
    /// after it stays current until the notice is stored.
    async fn lock_address(
        &mut self,
        email: &SubmitterEmail,
    ) -> Result<(), ContentNoticeSubmissionHistoryError>;

    /// The address's notices created after `after`.
    async fn submissions_after(
        &mut self,
        email: &SubmitterEmail,
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
