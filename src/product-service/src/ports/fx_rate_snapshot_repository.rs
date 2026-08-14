use common::error::boxed::BoxError;
use product_core::fx_rate_snapshot::FxRateSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxRateSnapshotInsertOutcome {
    Inserted,
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
        snapshot: &FxRateSnapshot,
        source_event_id: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError>;
}

pub trait FxRateSnapshotRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl FxRateSnapshotRepository + 'tx;
}
