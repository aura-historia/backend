use application::error::BoxError;
use user_core::{stripe_customer_id::StripeCustomerId, tier::UserTier, user_id::UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripeSubscriptionEvent {
    pub event_id: String,
    pub subscription_id: String,
    pub customer_id: StripeCustomerId,
    pub fingerprint: [u8; 32],
}

/// What an authenticated Stripe webhook delivery asks the reconciliation to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StripeWebhookEvent {
    /// A subscription created, updated or deleted event requesting a fresh provider read.
    Subscription(StripeSubscriptionEvent),
    /// Any other event type; acknowledged without effects.
    Ignored,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum StripeWebhookVerificationError {
    /// Missing, malformed, expired or non-matching `Stripe-Signature` proof.
    #[error("Stripe webhook signature is missing or invalid")]
    InvalidSignature,
    /// Authentic body that is not a supported event of the pinned API version.
    #[error("Stripe webhook event is malformed")]
    MalformedEvent,
    #[error("Stripe webhook signature could not be computed")]
    CryptographicFailure,
}

/// Authenticates a Stripe webhook delivery and parses its event.
pub trait StripeWebhookVerifier: Send + Sync {
    /// `signature_header` is the single received `Stripe-Signature` value and `raw_body` the
    /// exact received bytes. `arrived_at_unix_seconds` comes from the trusted runtime clock.
    /// The signature is checked before the body is parsed.
    fn verify(
        &self,
        signature_header: &[u8],
        raw_body: &[u8],
        arrived_at_unix_seconds: i64,
    ) -> Result<StripeWebhookEvent, StripeWebhookVerificationError>;
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
