use application::error::BoxError;
use shop_core::shop_id::ShopId;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum PartnerProductListingAuthorizationError {
    #[error("shop not found")]
    ShopNotFound,
    #[error("actor is not allowed to manage this shop's products")]
    Forbidden,
    #[error("partner product authorization is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product authorization failed internally")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait PartnerProductListingAuthorizer: Send {
    async fn authorize(
        &mut self,
        actor_id: UserId,
        shop_id: ShopId,
    ) -> Result<(), PartnerProductListingAuthorizationError>;
}

pub trait PartnerProductListingAuthorizerFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnerProductListingAuthorizer + 'tx;
}
