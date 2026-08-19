use common::error::boxed::BoxError;
use fxrate_core::{FxRateId, FxRateSnapshot, NewFxRateSnapshot};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FxRateSnapshotInsertOutcome {
    Inserted(FxRateSnapshot),
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum FxRateSnapshotRepositoryError {
    #[error("FX rate snapshot insert failed")]
    InsertFailed {
        #[source]
        source: BoxError,
    },
    #[error("FX rate snapshot read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted FX rate snapshot is invalid")]
    InvalidPersistedSnapshot {
        #[source]
        source: BoxError,
    },
    #[error(
        "FX rate snapshot captured-at timestamp is not strictly newer than the canonical snapshot"
    )]
    CapturedAtNotMonotonic,
}

#[async_trait::async_trait]
pub trait FxRateSnapshotRepository: Send {
    async fn find_latest(
        &mut self,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>;

    async fn find_latest_at_or_before(
        &mut self,
        timestamp: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>;

    async fn find_by_id(
        &mut self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError>;

    async fn find_by_ids(
        &mut self,
        ids: &[FxRateId],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError>;

    async fn insert(
        &mut self,
        snapshot: &NewFxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>;
}

pub trait FxRateSnapshotRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx;
}
