use crate::core::{
    partner_status::ShopPartnerStatus, shop_aggregate::Shop, shop_version::ShopVersion,
};
use common::operation_context::OperationContext;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusCommand {
    pub shop_id: ShopId,
    pub partner_status: ShopPartnerStatus,
    pub expected_version: Option<ShopVersion>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub partner_status: ShopPartnerStatus,
    pub version: ShopVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeShopPartnerStatusError {
    #[error("shop not found")]
    NotFound,
    #[error("shop version conflict")]
    VersionConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait ChangeShopPartnerStatusUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError>;
}

impl ChangeShopPartnerStatusResult {
    pub fn from_shop_and_version(shop: &Shop, version: ShopVersion) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            partner_status: shop.partner_status(),
            version,
        }
    }
}
