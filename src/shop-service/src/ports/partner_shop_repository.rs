#![allow(dead_code)]

use common::error::boxed::BoxError;
use common::user_id::UserId;
use shop_core::shop_id::ShopId;

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopRepositoryError {
    #[error("user not found")]
    UserNotFound {
        #[source]
        source: BoxError,
    },
    #[error("shop not found")]
    ShopNotFound {
        #[source]
        source: BoxError,
    },
    #[error("temporary partner shop persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnerShopRepository: Send {
    async fn grant(
        &mut self,
        user_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), PartnerShopRepositoryError>;
}

pub trait PartnerShopRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartnerShopRepository + 'tx;
}
