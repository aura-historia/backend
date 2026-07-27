use common::operation_context::OperationContext;
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopRequest {
    pub user_id: UserId,
    pub shop_id: ShopId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserPartnerShopResult {
    pub user_id: UserId,
    pub shop_id: ShopId,
    pub is_partner: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserPartnerShopError {
    #[error("operation not permitted")]
    Forbidden,
}

#[async_trait::async_trait]
pub trait CheckUserPartnerShopUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserPartnerShopRequest,
    ) -> Result<CheckUserPartnerShopResult, CheckUserPartnerShopError>;
}
