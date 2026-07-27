use crate::core::{partner_status::ShopPartnerStatus, shop_aggregate::Shop};
use common::operation_context::OperationContext;
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusCommand {
    pub shop_id: ShopId,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeShopPartnerStatusResult {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
    pub partner_status: ShopPartnerStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeShopPartnerStatusError {}

#[async_trait::async_trait]
pub trait ChangeShopPartnerStatusUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeShopPartnerStatusCommand,
    ) -> Result<ChangeShopPartnerStatusResult, ChangeShopPartnerStatusError>;
}

impl From<&Shop> for ChangeShopPartnerStatusResult {
    fn from(shop: &Shop) -> Self {
        Self {
            shop_id: shop.id(),
            shop_slug_id: shop.slug_id().clone(),
            name: shop.name().clone(),
            partner_status: shop.partner_status(),
        }
    }
}
