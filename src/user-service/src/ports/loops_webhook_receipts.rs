use application::error::BoxError;
use serde_email::Email;
use time::OffsetDateTime;

/// Durable delivery deduplication and a mailbox-scoped provider ordering fence.
/// These are operational records, not consent history. The preference fence must
/// outlive receipt cleanup so replaying an old event cannot restore permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopsWebhookReceiptDisposition {
    AppliedWithdrawal,
    AppliedContactRemoval,
    AppliedComplaintBlock,
    AppliedResubscription,
    IgnoredUnsupportedEvent,
    IgnoredInvalidRecipient,
    IgnoredUntrustworthyTime,
    IgnoredUnrelatedList,
    IgnoredApiEcho,
    IgnoredHardBounce,
    IgnoredContactMismatch,
    IgnoredStale,
    IgnoredNoRegisteredUser,
    IgnoredProviderState,
}

impl LoopsWebhookReceiptDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AppliedWithdrawal => "APPLIED_WITHDRAWAL",
            Self::AppliedContactRemoval => "APPLIED_CONTACT_REMOVAL",
            Self::AppliedComplaintBlock => "APPLIED_COMPLAINT_BLOCK",
            Self::AppliedResubscription => "APPLIED_RESUBSCRIPTION",
            Self::IgnoredUnsupportedEvent => "IGNORED_UNSUPPORTED_EVENT",
            Self::IgnoredInvalidRecipient => "IGNORED_INVALID_RECIPIENT",
            Self::IgnoredUntrustworthyTime => "IGNORED_UNTRUSTWORTHY_TIME",
            Self::IgnoredUnrelatedList => "IGNORED_UNRELATED_LIST",
            Self::IgnoredApiEcho => "IGNORED_API_ECHO",
            Self::IgnoredHardBounce => "IGNORED_HARD_BOUNCE",
            Self::IgnoredContactMismatch => "IGNORED_CONTACT_MISMATCH",
            Self::IgnoredStale => "IGNORED_STALE",
            Self::IgnoredNoRegisteredUser => "IGNORED_NO_REGISTERED_USER",
            Self::IgnoredProviderState => "IGNORED_PROVIDER_STATE",
        }
    }
}

/// The stored digest is sufficient to distinguish a retry from delivery-ID reuse.
/// It deliberately does not expose or retain the raw provider payload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LoopsWebhookReceiptLookup {
    pub raw_body_sha256: [u8; 32],
}

/// Persisted event receipt. No Debug implementation: provider identity and mailbox
/// details must not be included in logs.
pub struct LoopsWebhookReceiptInput {
    pub delivery_id: String,
    pub raw_body_sha256: [u8; 32],
    pub event_name: String,
    pub event_time: OffsetDateTime,
    pub provider_contact_id: Option<String>,
    pub email: Option<String>,
    pub mailing_list_id: Option<String>,
    pub disposition: LoopsWebhookReceiptDisposition,
    pub processed_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopsWebhookReceiptWriteOutcome {
    Inserted,
    ExistingSameDigest,
    ExistingDifferentDigest,
}

/// Durable current provider ordering/contact binding for one exact mailbox.
/// This record is independent of expiring delivery receipts.
#[derive(Clone, PartialEq, Eq)]
pub struct LoopsPreferenceFence {
    pub latest_event_at: OffsetDateTime,
    pub provider_contact_id: String,
    pub purpose_subscribed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LoopsWebhookReceiptError {
    #[error("invalid persisted Loops webhook state")]
    InvalidPersistedState,
    #[error("Loops webhook state changed concurrently")]
    ConcurrencyConflict,
    #[error("Loops webhook persistence unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
}

/// All methods use the caller's short PostgreSQL transaction. The caller must
/// hold the shared per-recipient lock before reading or advancing a preference
/// fence. Receipts are inserted only after the resulting application is ready to
/// commit in that same transaction.
#[async_trait::async_trait]
pub trait LoopsWebhookReceipts: Send {
    async fn find_by_delivery_id(
        &mut self,
        delivery_id: &str,
    ) -> Result<Option<LoopsWebhookReceiptLookup>, LoopsWebhookReceiptError>;

    async fn find_preference_fence(
        &mut self,
        email: &Email,
    ) -> Result<Option<LoopsPreferenceFence>, LoopsWebhookReceiptError>;

    async fn advance_preference_fence(
        &mut self,
        email: &Email,
        provider_contact_id: &str,
        event_at: OffsetDateTime,
        purpose_subscribed: bool,
    ) -> Result<(), LoopsWebhookReceiptError>;

    async fn insert_receipt(
        &mut self,
        receipt: LoopsWebhookReceiptInput,
    ) -> Result<LoopsWebhookReceiptWriteOutcome, LoopsWebhookReceiptError>;
}

pub trait LoopsWebhookReceiptsFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl LoopsWebhookReceipts + 'tx;
}
