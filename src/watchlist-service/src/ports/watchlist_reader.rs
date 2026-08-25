use product_listing_core::product_listing_id::ProductListingId;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::watchlist_state::WatchlistState;

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistProductListingView {
    pub user_id: UserId,
    pub product_listing_id: ProductListingId,
    pub notifications: bool,
    pub state: WatchlistState,
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
    ) -> Result<Vec<WatchlistProductListingView>, WatchlistReadError>;
    async fn find_user_ids_for_product(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Vec<UserId>, WatchlistReadError>;
}

pub trait WatchlistReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl WatchlistReader + 'tx;
}
