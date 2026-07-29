use crate::ports::{WatchlistRepository, WatchlistRepositoryError, WatchlistRepositoryFactory};
use common::operation_context::{OperationContext, Principal};
use common::product_id::ProductId;
use common::resource_state::domain::ResourceState;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use watchlist_core::{NewWatchlistProduct, WatchlistProduct};

#[derive(Debug, Clone, PartialEq)]
pub struct WatchProductCommand {
    pub user_id: UserId,
    pub product_id: ProductId,
    pub notifications: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchProductResult {
    pub entry: WatchlistProduct,
}

#[derive(Debug, thiserror::Error)]
pub enum WatchProductError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("watchlist entry already exists")]
    AlreadyExists,
    #[error("temporary watchlist persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted watchlist state")]
    InvalidPersistedState,
    #[error("failed to begin watchlist transaction")]
    BeginTransactionFailed,
    #[error("failed to commit watchlist transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait WatchProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WatchProductCommand,
    ) -> Result<WatchProductResult, WatchProductError>;
}

pub struct WatchProductHandler<U, R> {
    unit_of_work: U,
    watchlist: R,
}

impl<U, R> WatchProductHandler<U, R> {
    pub fn new(unit_of_work: U, watchlist: R) -> Self {
        Self {
            unit_of_work,
            watchlist,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> WatchProductUseCase for WatchProductHandler<U, R>
where
    U: UnitOfWork,
    R: WatchlistRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "watch_product", skip_all, fields(user_id = %command.user_id, product_id = %command.product_id, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: WatchProductCommand,
    ) -> Result<WatchProductResult, WatchProductError> {
        if matches!(context.principal, Principal::Anonymous) {
            return Err(WatchProductError::AuthenticatedActorRequired);
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WatchProductError::BeginTransactionFailed)?;
        if self
            .watchlist
            .in_transaction(&mut tx)
            .find_by_user_and_product(command.user_id, command.product_id)
            .await?
            .is_some()
        {
            return Err(WatchProductError::AlreadyExists);
        }

        let entry = WatchlistProduct::create(NewWatchlistProduct {
            user_id: command.user_id,
            product_id: command.product_id,
            notifications: command.notifications,
            state: ResourceState::Active,
        });
        self.watchlist
            .in_transaction(&mut tx)
            .insert(&entry)
            .await?;
        tx.commit()
            .await
            .map_err(|_| WatchProductError::CommitTransactionFailed)?;
        Ok(WatchProductResult { entry })
    }
}

impl From<WatchlistRepositoryError> for WatchProductError {
    fn from(value: WatchlistRepositoryError) -> Self {
        match value {
            WatchlistRepositoryError::AlreadyExists => WatchProductError::AlreadyExists,
            WatchlistRepositoryError::InvalidPersistedState => {
                WatchProductError::InvalidPersistedState
            }
            WatchlistRepositoryError::LookupFailed
            | WatchlistRepositoryError::InsertFailed
            | WatchlistRepositoryError::UpdateFailed
            | WatchlistRepositoryError::DeleteFailed => WatchProductError::TemporarilyUnavailable,
        }
    }
}
