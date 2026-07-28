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
pub trait PartnerShopReader: Send {
    async fn is_user_partner_of_shop(
        &mut self,
        request: &CheckUserPartnerShopRequest,
    ) -> Result<bool, PartnerShopReadError>;
}

pub trait PartnerShopReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnerShopReader + 'tx;
}
