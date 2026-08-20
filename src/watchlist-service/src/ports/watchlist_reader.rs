use common::resource_state::domain::ResourceState;
use common::user_id::UserId;
use product_core::product_id::ProductId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProductView {
    pub user_id: UserId,
    pub product_id: ProductId,
    pub notifications: bool,
    pub state: ResourceState,
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
