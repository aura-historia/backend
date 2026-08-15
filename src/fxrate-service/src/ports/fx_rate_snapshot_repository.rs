use common::error::boxed::BoxError;
use fxrate_core::{FxRateSnapshot, NewFxRateSnapshot};

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
}

#[async_trait::async_trait]
pub trait FxRateSnapshotRepository: Send {
    async fn insert(
        &mut self,
        snapshot: &NewFxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>;
}

pub trait FxRateSnapshotRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx;
}
