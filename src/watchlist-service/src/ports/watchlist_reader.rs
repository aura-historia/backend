use common::product_id::ProductId;
use common::user_id::UserId;
use time::OffsetDateTime;
use watchlist_core::WatchlistProduct;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProductView {
    pub entry: WatchlistProduct,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchlistReadError {
    #[error("watchlist read failed")]
    ReadFailed,
    #[error("persisted watchlist state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait WatchlistReader: Send {
    async fn find_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<WatchlistProductView>, WatchlistReadError>;
    async fn find_user_ids_for_product(
        &mut self,
        product_id: ProductId,
    ) -> Result<Vec<UserId>, WatchlistReadError>;
}

pub trait WatchlistReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WatchlistReader + 'tx;
}
