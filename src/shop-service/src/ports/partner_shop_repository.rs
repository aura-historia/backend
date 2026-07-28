#![allow(dead_code)]

use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, thiserror::Error)]
pub enum PartnerShopRepositoryError {
    #[error("user not found")]
    UserNotFound,
    #[error("shop not found")]
    ShopNotFound,
    #[error("temporary partner shop persistence failure")]
    TemporarilyUnavailable,
    #[error("internal partner shop persistence failure")]
    Internal,
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
