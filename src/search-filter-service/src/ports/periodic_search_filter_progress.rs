use application::error::BoxError;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodicSearchFilterProgressLockOutcome {
    Current { matched_through: OffsetDateTime },
    AlreadyCovered,
    ChangedOrInactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodicSearchFilterProgressWriteOutcome {
    Advanced,
    AlreadyCovered,
    Superseded,
}

#[derive(Debug, thiserror::Error)]
pub enum PeriodicSearchFilterProgressError {
    #[error("periodic search-filter progress read or write failed")]
    PersistenceFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PeriodicSearchFilterProgress: Send {
    async fn lock_and_read(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        created: OffsetDateTime,
        window_end: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressError>;

    async fn compare_and_set(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected_matched_through: OffsetDateTime,
        matched_through: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressWriteOutcome, PeriodicSearchFilterProgressError>;
}

pub trait PeriodicSearchFilterProgressFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PeriodicSearchFilterProgress + 'tx;
}
