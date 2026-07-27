use common::operation_context::OperationContext;
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopCommand {
    pub user_id: UserId,
    pub shop_id: ShopId,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantPartnerShopError {
    #[error("user not found")]
    NotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait GrantPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError>;
}
