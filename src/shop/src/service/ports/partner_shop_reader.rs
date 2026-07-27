#![allow(dead_code)]

use crate::service::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopReadError {
    #[error("invalid partner shop read model")]
    InvalidReadModel,
}

#[async_trait::async_trait]
pub(crate) trait PartnerShopReader: Send + Sync {
    async fn is_user_partner_of_shop(
        &self,
        request: &CheckUserPartnerShopRequest,
    ) -> Result<bool, PartnerShopReadError>;
}
