#![allow(dead_code)]

use crate::ports::PartnerShopApplicationRepositoryError;
use shop_core::shop_id::ShopId;
use shop_partner_core::partner_shop_application::PartnerShopApplicationPayload;
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_partner_core::partner_shop_application_state::PartnerShopApplicationState;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopApplicationView {
    pub id: PartnerShopApplicationId,
    pub applicant_user_id: UserId,
    pub business_state: PartnerShopApplicationState,
    pub payload: PartnerShopApplicationPayload,
    pub shop_id: ShopId,
}

#[async_trait::async_trait]
pub trait PartnerShopApplicationReader: Send {
    async fn list_all(
        &mut self,
    ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError>;

    async fn list_by_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<PartnerShopApplicationView>, PartnerShopApplicationRepositoryError>;
}

pub trait PartnerShopApplicationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnerShopApplicationReader + 'tx;
}
