use application::error::BoxError;
use domain_primitives::versioned::Versioned;
use product_listing_core::product_id::ProductId;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProduct;

domain_primitives::version_newtype!(WatchlistStorageVersion);

pub type VersionedWatchlistProduct = Versioned<WatchlistProduct, WatchlistStorageVersion>;

#[derive(Debug, thiserror::Error)]
pub enum WatchlistRepositoryError {
    #[error("concurrent watchlist entry update")]
    ConcurrencyConflict,
    #[error("watchlist entry already exists")]
    AlreadyExists,
    #[error("watchlist entry lookup failed")]
    LookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist entry insert failed")]
    InsertFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist entry update failed")]
    UpdateFailed {
        #[source]
        source: BoxError,
    },
    #[error("watchlist entry delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::static_error;
    use std::error::Error;

    #[test]
    fn should_preserve_repository_query_failure_source() {
        let error = WatchlistRepositoryError::UpdateFailed {
            source: static_error("database connection lost"),
        };

        assert_eq!(
            Some("database connection lost"),
            error.source().map(ToString::to_string).as_deref()
        );
    }
}
