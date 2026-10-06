use application::error::BoxError;
use serde_email::Email;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::user_id::UserId;

use super::UserStorageVersion;

/// Opaque per-mailbox serialization key, not an email equivalence or ownership proof.
/// Preserve plus tags and dots; the original exact email remains the provider identity.
pub fn marketing_consent_recipient_key(email: &Email) -> String {
    use std::fmt::Write;
    let mut digest = Sha256::new();
    digest.update(b"aura:marketing-consent:recipient:v1\0");
    let address: &str = email.as_ref();
    digest.update(
        address
            .bytes()
            .map(|byte| byte.to_ascii_lowercase())
            .collect::<Vec<_>>(),
    );
    let mut result = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentIntentSource {
    CognitoSignup,
    AuraDoubleOptIn,
    UserWithdrawal,

    UserDeletion,
    EmailOnlyWithdrawal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentSubject {
    User(UserId),
    EmailOnly,
}

// Intentionally no Debug: exact recipient and proof key must not reach logs.
#[derive(Clone)]
pub struct ConsentIntent {
    pub intent_id: MarketingConsentSyncIntentId,
    pub source_key: String,
    pub subject: ConsentSubject,
    pub source: ConsentIntentSource,
    pub email: Email,

    pub desired: bool,
}

#[derive(Clone)]
pub struct ConsentUser {
    pub user_id: UserId,
    pub email: Email,
    pub version: UserStorageVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum MarketingConsentIntentError {
    #[error("concurrent consent change or user missing")]
    ConcurrencyConflict,
    #[error("consent source key conflict")]
    SourceKeyConflict,
    #[error("registered email cannot be used for an email-only intent")]
    RegisteredEmail,
    #[error("invalid consent intent input")]
    InvalidInput,
    #[error("invalid persisted consent state")]
    InvalidPersistedState,
    #[error("consent persistence unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
}

/// All operations share the caller's short transaction. Implementations serialize
/// recipient mutations and enforce exact email/version checks and source-key replay.
/// No method here may perform provider I/O.
#[async_trait::async_trait]
pub trait MarketingConsentIntents: Send {
    async fn find_by_source_key(
        &mut self,
        key: &str,
    ) -> Result<Option<ConsentIntent>, MarketingConsentIntentError>;
    async fn find_user_by_id(
        &mut self,
        id: UserId,
    ) -> Result<Option<ConsentUser>, MarketingConsentIntentError>;
    /// Exact canonical account-email match; never an authenticated actor lookup.
    async fn find_user_by_email(
        &mut self,
        email: &Email,
    ) -> Result<Option<ConsentUser>, MarketingConsentIntentError>;
    async fn record_user_transition(
        &mut self,
        user: &ConsentUser,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
    async fn record_email_only_intent(
        &mut self,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
    /// A provider-originated withdrawal updates User consent/revision without an outbound intent.
    async fn apply_provider_withdrawal(
        &mut self,
        user: &ConsentUser,
        changed_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError>;
    /// Invalidate unsent and leased grants for this recipient; do not enqueue an echo.
    async fn cancel_provider_backsync(
        &mut self,
        email: &Email,
    ) -> Result<(), MarketingConsentIntentError>;

    async fn record_user_deletion(
        &mut self,
        user: &ConsentUser,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
}

pub trait MarketingConsentIntentsFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl MarketingConsentIntents + 'tx;
}
