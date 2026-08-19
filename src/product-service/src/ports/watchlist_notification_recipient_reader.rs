use common::{error::boxed::BoxError, product_id::ProductId, user_id::UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchlistNotificationRecipient {
    pub user_id: UserId,
    pub external_delivery_requested: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchlistNotificationRecipientReadError {
    #[error("watchlist notification recipient query failed")]
    QueryFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait WatchlistNotificationRecipientReader: Send {
    async fn find_active_for_product(
        &mut self,
        product_id: ProductId,
    ) -> Result<Vec<WatchlistNotificationRecipient>, WatchlistNotificationRecipientReadError>;
}

pub trait WatchlistNotificationRecipientReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl WatchlistNotificationRecipientReader + 'tx;
}
