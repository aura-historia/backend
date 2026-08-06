use common::postgres::SqlxTransaction;
use common::user_id::UserId;
use watchlist_service::ports::{
    WatchlistQuotaReadError, WatchlistQuotaReader, WatchlistQuotaReaderFactory,
};

#[derive(Debug, Clone, Default)]
pub struct SqlxWatchlistQuotaReaderFactory;

struct SqlxWatchlistQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl WatchlistQuotaReaderFactory<SqlxTransaction> for SqlxWatchlistQuotaReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WatchlistQuotaReader + 'tx {
        SqlxWatchlistQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistQuotaReader for SqlxWatchlistQuotaReader<'_> {
    async fn count_active_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<usize, WatchlistQuotaReadError> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM product_watchlist WHERE user_id = $1 AND state = 'Active'",
        )
        .bind(uuid::Uuid::from(user_id))
        .fetch_one(self.tx.connection())
        .await
        .map_err(|_| WatchlistQuotaReadError::ReadFailed)?;
        usize::try_from(count).map_err(|_| WatchlistQuotaReadError::ReadFailed)
    }
}
