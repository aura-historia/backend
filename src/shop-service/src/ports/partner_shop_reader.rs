#![allow(dead_code)]

use crate::use_cases::queries::check_user_partner_shop::CheckUserPartnerShopRequest;
use crate::use_cases::queries::search_shops::ShopSummary;
use application::error::BoxError;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopReadError {
    #[error("temporary partner shop read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partner shop read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnerShopReader: Send {
    async fn is_user_partner_of_shop(
        &mut self,
        request: &CheckUserPartnerShopRequest,
    ) -> Result<bool, PartnerShopReadError>;

    async fn list_summaries_for_user(
        &mut self,
        user_id: UserId,
    ) -> Result<Vec<ShopSummary>, PartnerShopReadError>;
}

pub trait PartnerShopReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnerShopReader + 'tx;
}
