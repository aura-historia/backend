use common::operation_context::OperationContext;
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopCommand {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, thiserror::Error)]
pub enum GrantPartnerShopError {
    #[error("user not found")]
    UserNotFound,
    #[error("shop not found")]
    ShopNotFound,
    #[error("operation not permitted")]
    Forbidden,
}

#[async_trait::async_trait]
pub trait GrantPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartnerShopCommand,
    ) -> Result<GrantPartnerShopResult, GrantPartnerShopError>;
}
