use crate::service::ports::product_event_store::ProductEventStoreError;
use crate::service::ports::product_repository::ProductRepositoryError;
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::product_id::ProductId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductCommand {
    pub product_id: ProductId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteProductResult {
    pub product_id: ProductId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductError {
    #[error("authenticated actor required to delete product")]
    AuthenticatedActorRequired,
    #[error("product not found")]
    ProductNotFound,
    #[error("product delete conflicted with a concurrent event")]
    ConcurrencyConflict,
    #[error("product repository unavailable")]
    ProductRepositoryUnavailable,
    #[error("product event store unavailable")]
    ProductEventStoreUnavailable,
    #[error("product event already exists")]
    EventConflict,
    #[error("failed to begin delete product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete product transaction")]
    CommitTransactionFailed,
    #[error("internal product repository failure")]
    ProductRepositoryInternal,
    #[error("internal product event store failure")]
    ProductEventStoreInternal,
}

#[async_trait::async_trait]
pub trait DeleteProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteProductCommand,
    ) -> Result<DeleteProductResult, DeleteProductError>;
}

impl From<ProductRepositoryError> for DeleteProductError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductRepositoryError::TemporarilyUnavailable => Self::ProductRepositoryUnavailable,
            ProductRepositoryError::ProductKeyConflict
            | ProductRepositoryError::SlugConflict
            | ProductRepositoryError::InvalidPersistedState
            | ProductRepositoryError::Internal => Self::ProductRepositoryInternal,
        }
    }
}

impl From<ProductEventStoreError> for DeleteProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::TemporarilyUnavailable => Self::ProductEventStoreUnavailable,
            ProductEventStoreError::EventConflict => Self::EventConflict,
            ProductEventStoreError::InvalidEvent | ProductEventStoreError::Internal => {
                Self::ProductEventStoreInternal
            }
        }
    }
}
