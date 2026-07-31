use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::shop_id::ShopId;

#[derive(Debug, thiserror::Error)]
pub enum ShopWritePolicyError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary shop write policy failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal shop write policy failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ShopWritePolicy: Send + Sync {
    async fn ensure_can_create_shop(
        &self,
        context: &OperationContext,
    ) -> Result<(), ShopWritePolicyError>;

    async fn ensure_can_update_shop(
        &self,
        context: &OperationContext,
        shop_id: ShopId,
    ) -> Result<(), ShopWritePolicyError>;
}
