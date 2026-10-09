use application::error::BoxError;
use user_core::{stripe_customer_id::StripeCustomerId, tier::UserTier, user_id::UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeSubscriptionEvent {
    pub event_id: String,
    pub subscription_id: String,
    pub customer_id: StripeCustomerId,
    pub fingerprint: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct StripeCustomerSubscriptionState {
    pub user_id: Option<UserId>,
    pub tier: UserTier,
}

#[derive(Debug, thiserror::Error)]
pub enum StripeSubscriptionSyncError {
    #[error("Stripe event identity was reused with different content")]
    ReceiptConflict,
    #[error("a newer Stripe reconciliation committed during the provider read")]
    ConcurrentReconciliation,
    #[error("invalid Stripe subscription event")]
    InvalidEvent,
    #[error("Stripe subscription state is unavailable")]
    Unavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid Stripe subscription state")]
    InvalidState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait StripeCustomerSubscriptionReader: Send + Sync {
    async fn read(
        &self,
        customer: &StripeCustomerId,
    ) -> Result<StripeCustomerSubscriptionState, StripeSubscriptionSyncError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripeSubscriptionEventObservation {
    Pending { revision: u64 },
    Applied,
}

#[async_trait::async_trait]
pub trait StripeSubscriptionEventStore: Send {
    /// Read under the customer reconciliation lock. Commit before provider I/O.
    async fn observe(
        &mut self,
        event: &StripeSubscriptionEvent,
    ) -> Result<StripeSubscriptionEventObservation, StripeSubscriptionSyncError>;

    /// Recheck the receipt and revision, then advance the revision and record the
    /// event in the same transaction as user and entitlement changes.
    async fn complete(
        &mut self,
        event: &StripeSubscriptionEvent,
        revision: u64,
    ) -> Result<StripeSubscriptionEventObservation, StripeSubscriptionSyncError>;
}

pub trait StripeSubscriptionEventStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl StripeSubscriptionEventStore + 'tx;
}
