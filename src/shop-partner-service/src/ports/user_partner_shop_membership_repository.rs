use common::error::boxed::BoxError;
use common::{shop_id::ShopId, user_id::UserId};

#[derive(Debug, thiserror::Error)]
pub enum UserPartnerShopMembershipRepositoryError {
    #[error("temporary user partner shop membership persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal user partner shop membership persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserPartnerShopMembershipRepository: Send {
    /// Grants partner-shop access once. Replaying the same grant is a no-op.
    async fn grant(
        &mut self,
        user_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), UserPartnerShopMembershipRepositoryError>;
}

pub trait UserPartnerShopMembershipRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl UserPartnerShopMembershipRepository + 'tx;
}
