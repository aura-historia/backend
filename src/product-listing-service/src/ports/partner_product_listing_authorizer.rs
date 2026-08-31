use application::error::BoxError;
use listing_source_core::ListingSourceId;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum PartnerProductListingAuthorizationError {
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("actor is not allowed to manage this listing source's products")]
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
        listing_source_id: ListingSourceId,
    ) -> Result<(), PartnerProductListingAuthorizationError>;
}

pub trait PartnerProductListingAuthorizerFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl PartnerProductListingAuthorizer + 'tx;
}
