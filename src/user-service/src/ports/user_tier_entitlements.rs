use common::error::boxed::BoxError;
use common::user_id::UserId;
use user_core::tier::UserTier;

#[derive(Debug, thiserror::Error)]
pub enum UserTierEntitlementsError {
    #[error("user tier entitlement lock failed")]
    LockFailed {
        #[source]
        source: BoxError,
    },
    #[error("user tier entitlement reconciliation failed")]
    ReconciliationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserTierEntitlements: Send {
    async fn lock_user_tier(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserTier>, UserTierEntitlementsError>;

    async fn reconcile_for_tier(
        &mut self,
        user_id: UserId,
        tier: UserTier,
    ) -> Result<(), UserTierEntitlementsError>;
}

pub trait UserTierEntitlementsFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserTierEntitlements + 'tx;
}
