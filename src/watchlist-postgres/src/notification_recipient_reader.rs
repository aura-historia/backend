use common::{postgres::SqlxTransaction, product_id::ProductId};
use product_service::ports::{
    WatchlistNotificationRecipient, WatchlistNotificationRecipientReadError,
    WatchlistNotificationRecipientReader, WatchlistNotificationRecipientReaderFactory,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxWatchlistNotificationRecipientReaderFactory;

struct SqlxWatchlistNotificationRecipientReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl WatchlistNotificationRecipientReaderFactory<SqlxTransaction>
    for SqlxWatchlistNotificationRecipientReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl WatchlistNotificationRecipientReader + 'tx {
        SqlxWatchlistNotificationRecipientReader { tx }
    }
}

#[async_trait::async_trait]
impl WatchlistNotificationRecipientReader for SqlxWatchlistNotificationRecipientReader<'_> {
    async fn find_active_for_product(
        &mut self,
        product_id: ProductId,
    ) -> Result<Vec<WatchlistNotificationRecipient>, WatchlistNotificationRecipientReadError> {
        sqlx::query_as::<_, (uuid::Uuid, bool)>(
            "SELECT user_id, notifications FROM product_watchlist WHERE product_id = $1 AND state = 'Active' ORDER BY user_id ASC",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_all(self.tx.connection())
        .await
        .map_err(|_| WatchlistNotificationRecipientReadError::QueryFailed)
        .map(|rows| rows.into_iter().map(|(user_id, external)| WatchlistNotificationRecipient {
            user_id: user_id.into(), external,
        }).collect())
    }
}
