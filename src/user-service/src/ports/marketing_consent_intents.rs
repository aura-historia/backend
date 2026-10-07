use application::error::BoxError;
use serde_email::Email;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::user_id::UserId;

use super::{NewsletterProfile, UserStorageVersion};

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
    ProviderRaceRepair,
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
    /// Confirmed DOI profile snapshot for asynchronous provider synchronization.
    pub profile_snapshot: Option<Box<NewsletterProfile>>,

    pub desired: bool,
}

#[derive(Clone)]
pub struct ConsentUser {
    pub user_id: UserId,
    pub email: Email,
    pub version: UserStorageVersion,
    pub marketing_email_consent: bool,
    pub marketing_email_consent_revision: i64,
    pub marketing_email_consent_changed_at: Option<OffsetDateTime>,
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
    /// Acquire the shared per-recipient advisory lock before a provider decision reads
    /// the current account row or invalidates any pending proof.
    async fn lock_recipient(&mut self, email: &Email) -> Result<(), MarketingConsentIntentError>;

    /// Acquire the same transaction-scoped source lock used by consent writes.
    async fn lock_source_key(&mut self, key: &str) -> Result<(), MarketingConsentIntentError>;

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
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
    async fn record_email_only_intent(
        &mut self,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
        source_key: &str,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
    /// A provider-originated withdrawal updates User consent/revision without an outbound intent.
    async fn apply_provider_withdrawal(
        &mut self,
        user: &ConsentUser,
        changed_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError>;
    /// Apply an explicitly verified provider resubscription to a registered exact
    /// mailbox without creating an outbound synchronization intent. Implementations
    /// must fence this update with the supplied User version.
    async fn apply_provider_resubscription(
        &mut self,
        _user: &ConsentUser,
        _changed_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError> {
        Err(MarketingConsentIntentError::InvalidInput)
    }
    /// Invalidate unsent and leased grants for this recipient; do not enqueue an echo.
    async fn cancel_provider_backsync(
        &mut self,
        email: &Email,
    ) -> Result<(), MarketingConsentIntentError>;

    /// Cancel unconfirmed DOI challenges in the provider-withdrawal transaction.
    async fn invalidate_newsletter_confirmation_challenges(
        &mut self,
        email: &Email,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), MarketingConsentIntentError>;

    /// Decide and persist a correction for an already leased grant invalidated by
    /// provider back-sync. The implementation locks the original recipient and
    /// rechecks authoritative state; it never performs provider I/O.
    async fn repair_raced_grant_if_needed(
        &mut self,
        original: MarketingConsentSyncIntentId,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<GrantRaceRepairOutcome, MarketingConsentIntentError>;

    async fn record_user_deletion(
        &mut self,
        user: &ConsentUser,
        source_key: &str,
        changed_at: OffsetDateTime,
    ) -> Result<ConsentIntent, MarketingConsentIntentError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantRaceRepairOutcome {
    NoRepairNeeded,
    ExistingRepair(MarketingConsentSyncIntentId),
    RepairScheduled(MarketingConsentSyncIntentId),
}

/// A committed lease over an immutable target. Never log this value: the target
/// contains the exact provider email and the source proof key.
#[derive(Clone)]
pub struct ConsentWorkerClaim {
    pub intent: ConsentIntent,
    pub recipient_key: String,
    pub consent_revision: Option<i64>,
    pub not_after: Option<OffsetDateTime>,
    pub changed_at: OffsetDateTime,
    pub lease_token: String,
    pub lease_expires_at: OffsetDateTime,
    pub attempt_count: u32,
    /// `true` only when this lease recovered an expired IN_PROGRESS attempt whose provider
    /// outcome was never durably classified. The worker must reconcile before any new grant.
    pub prior_attempt_write_ambiguous: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentWorkerTerminalStatus {
    Applied,
    Superseded,
    Blocked,
    Failed,
}

pub enum ConsentWorkerClaimOutcome {
    Claimed(ConsentWorkerClaim),
    Missing,
    Terminal(ConsentWorkerTerminalStatus),
    /// An active lease has custody; do not acknowledge or send.
    Deferred {
        lease_expires_at: OffsetDateTime,
    },
}

pub enum ConsentWorkerRecheckOutcome {
    Ready(ConsentIntent),
    Missing,
    Terminal(ConsentWorkerTerminalStatus),
    /// The lease is expired, replaced, or otherwise no longer owned.
    LeaseLost,
}

#[derive(Clone, Copy)]
pub enum ConsentWorkerFinalization<'a> {
    Applied {
        provider_contact_id: Option<&'a str>,
    },
    Failed {
        error_code: &'a str,
    },
    Blocked {
        error_code: &'a str,
    },
}

/// Transaction-bound worker operations. Commit the claim before provider I/O;
/// recheck before and after it in separate short transactions. Never hold a
/// transaction over network I/O. A false finalize means no matching receipt or
/// live lease; only an exact committed token/result/timestamp retry returns true.
#[async_trait::async_trait]
pub trait MarketingConsentIntentWorker<Tx>: Send + Sync {
    async fn claim_by_id(
        &self,
        tx: &mut Tx,
        id: MarketingConsentSyncIntentId,
    ) -> Result<ConsentWorkerClaimOutcome, MarketingConsentIntentError>;
    async fn recheck(
        &self,
        tx: &mut Tx,
        claim: &ConsentWorkerClaim,
    ) -> Result<ConsentWorkerRecheckOutcome, MarketingConsentIntentError>;
    async fn finalize_claim(
        &self,
        tx: &mut Tx,
        claim: &ConsentWorkerClaim,
        result: ConsentWorkerFinalization<'_>,
        completed_at: OffsetDateTime,
    ) -> Result<bool, MarketingConsentIntentError>;

    /// Return a definitively non-writing attempt to PENDING so its exact wake-up can retry.
    /// The implementation records a retry marker which distinguishes this case from an
    /// abandoned lease when the next claim is made. Repeating the same lease/reason after an
    /// unconfirmed commit must confirm the matching marker or reapply the release if it rolled
    /// back; an older claim must not confirm a later attempt's marker.
    async fn release_for_retry(
        &self,
        tx: &mut Tx,
        claim: &ConsentWorkerClaim,
        reason_code: &str,
    ) -> Result<bool, MarketingConsentIntentError>;
}

pub trait MarketingConsentIntentsFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl MarketingConsentIntents + 'tx;
}
