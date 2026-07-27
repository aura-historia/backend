use common::operation_context::OperationContext;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsResult {
    pub user_id: UserId,
    pub items: Vec<PartnerShopSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListPartnerShopsError {
    #[error("user not found")]
    UserNotFound,
    #[error("operation not permitted")]
    Forbidden,
}

#[async_trait::async_trait]
pub trait ListPartnerShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError>;
}
