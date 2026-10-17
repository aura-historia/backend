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
    async fn load_selection(
        &mut self,
        window_end: OffsetDateTime,
        search_filter_id: UserSearchFilterId,
    ) -> Result<
        Option<Vec<product_listing_service::ports::ProductListingSearchFilterMatchSourceRef>>,
        PeriodicSearchFilterProgressError,
    >;
    /// First selection wins, including an empty selection. Returns its durable value.
    async fn freeze_selection(
        &mut self,
        window_end: OffsetDateTime,
        search_filter_id: UserSearchFilterId,
        selected: &[product_listing_service::ports::ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        Vec<product_listing_service::ports::ProductListingSearchFilterMatchSourceRef>,
        PeriodicSearchFilterProgressError,
    >;

    /// Freeze the intended filters atomically, or resume the unfinished logical run.
    async fn begin_run(
        &mut self,
        proposed_window_end: OffsetDateTime,
    ) -> Result<OffsetDateTime, PeriodicSearchFilterProgressError>;
    async fn record_filter_attempt(
        &mut self,
        window_end: OffsetDateTime,
        search_filter_id: UserSearchFilterId,
        completed: bool,
    ) -> Result<(), PeriodicSearchFilterProgressError>;
    async fn finish_run(
        &mut self,
        window_end: OffsetDateTime,
    ) -> Result<(), PeriodicSearchFilterProgressError>;

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
