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
    #[error("product not found")]
    NotFound,
    #[error("concurrent product update")]
    ConcurrencyConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait DeleteProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteProductCommand,
    ) -> Result<DeleteProductResult, DeleteProductError>;
}
