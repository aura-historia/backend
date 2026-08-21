use common::product_id::ProductId;
use common::user_id::UserId;
use common::versioned::Versioned;
use watchlist_core::WatchlistProduct;

common::version_newtype!(WatchlistStorageVersion);

pub type VersionedWatchlistProduct = Versioned<WatchlistProduct, WatchlistStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum WatchlistRepositoryError {
    #[error("concurrent watchlist entry update")]
    ConcurrencyConflict,
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
    ) -> Result<Option<VersionedWatchlistProduct>, WatchlistRepositoryError>;

    async fn insert(
        &mut self,
        entry: &WatchlistProduct,
    ) -> Result<VersionedWatchlistProduct, WatchlistRepositoryError>;

    async fn update(
        &mut self,
        entry: &WatchlistProduct,
        expected_version: WatchlistStorageVersion,
    ) -> Result<VersionedWatchlistProduct, WatchlistRepositoryError>;

    async fn delete(
        &mut self,
        user_id: UserId,
        product_id: ProductId,
        expected_version: WatchlistStorageVersion,
    ) -> Result<(), WatchlistRepositoryError>;
}

pub trait WatchlistRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WatchlistRepository + 'tx;
}
