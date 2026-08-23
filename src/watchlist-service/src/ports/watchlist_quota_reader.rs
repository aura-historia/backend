use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum WatchlistQuotaReadError {
    #[error("watchlist quota read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait WatchlistQuotaReader: Send {
    async fn count_active_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<usize, WatchlistQuotaReadError>;
}

pub trait WatchlistQuotaReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WatchlistQuotaReader + 'tx;
}
