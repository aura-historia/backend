use common::{error::boxed::BoxError, fx_rate_id::FxRateId};
use fxrate_core::FxRateSnapshot;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum FxRateSnapshotReadError {
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
}

#[async_trait::async_trait]
pub trait FxRateSnapshotReader: Send + Sync {
    async fn latest(&self) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError>;

    async fn latest_at_or_before(
        &self,
        timestamp: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError>;

    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError>;

    async fn find_by_ids(
        &self,
        ids: &[FxRateId],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError>;
}
