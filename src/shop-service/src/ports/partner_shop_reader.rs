#![allow(dead_code)]

use crate::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopReadError {
    #[error("temporary partner shop read failure")]
    TemporarilyUnavailable,
    #[error("invalid partner shop read model")]
    InvalidReadModel,
    #[error("internal partner shop read failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait PartnerShopReader: Send + Sync {
    async fn is_user_partner_of_shop(
        &self,
        request: &CheckUserPartnerShopRequest,
    ) -> Result<bool, PartnerShopReadError>;
}
