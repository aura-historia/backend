use common::user_id::UserId;
use product_core::product_id::ProductId;
use watchlist_core::WatchlistProduct;

#[derive(Debug, thiserror::Error)]
pub enum WatchlistRepositoryError {
    #[error("watchlist entry already exists")]
    AlreadyExists,
    #[error("watchlist entry lookup failed")]
    LookupFailed,
    #[error("watchlist entry insert failed")]
    InsertFailed,
    #[error("watchlist entry update failed")]
    UpdateFailed,
    #[error("watchlist entry delete failed")]
    DeleteFailed,
    #[error("persisted watchlist state is invalid")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait WatchlistRepository: Send {
    async fn find_by_user_and_product(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
    ) -> Result<Option<WatchlistProduct>, WatchlistRepositoryError>;

    async fn insert(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<WatchlistProduct, WatchlistRepositoryError>;

    async fn update(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<WatchlistProduct, WatchlistRepositoryError>;

    async fn delete(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
    ) -> Result<(), WatchlistRepositoryError>;
}

pub trait WatchlistRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WatchlistRepository + 'tx;
}
