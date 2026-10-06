use crate::ports::NewsletterProfile;
use application::error::BoxError;
use serde_email::Email;
use time::OffsetDateTime;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::newsletter_confirmation::{
    NewsletterConfirmationTokenDigest, RawNewsletterConfirmationToken,
};
use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
use user_core::user_id::UserId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewsletterConfirmationSendStatus {
    NotAttempted,
    Accepted,
    DefinitelyRejected,
    AcceptanceUnknown,
}

/// Sensitive operational state. Deliberately has no Debug implementation.
#[derive(Clone)]
pub struct NewsletterConfirmationChallenge {
    pub id: NewsletterConfirmationId,
    pub token_digest: NewsletterConfirmationTokenDigest,
    pub email: Email,
    pub recipient_key: String,
    /// Authoritative same-mailbox User at issuance, or a binding made at confirmation.
    pub bound_user_id: Option<UserId>,
    /// The authenticated actor, retained only as request metadata.
    pub requested_by_user_id: Option<UserId>,
    pub profile: NewsletterProfile,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
    pub send_status: NewsletterConfirmationSendStatus,
    pub confirmed_at: Option<OffsetDateTime>,
    pub invalidated_at: Option<OffsetDateTime>,
    pub resulting_intent_id: Option<MarketingConsentSyncIntentId>,
}

/// PII-bearing write input. Deliberately has no Debug implementation.
pub struct NewNewsletterConfirmationChallenge {
    pub id: NewsletterConfirmationId,
    pub token_digest: NewsletterConfirmationTokenDigest,
    pub email: Email,
    pub requested_by_user_id: Option<UserId>,
    pub profile: NewsletterProfile,
    pub now: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewsletterConfirmationIssueOutcome {
    Issued,
    Suppressed,
}

#[derive(Debug, thiserror::Error)]
pub enum NewsletterConfirmationChallengeError {
    #[error("invalid newsletter confirmation challenge input")]
    InvalidInput,
    #[error("invalid persisted newsletter confirmation state")]
    InvalidPersistedState,
    #[error("newsletter confirmation persistence unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
}

/// Transaction-bound challenge operations. Issuance, confirmation and cancellation
/// use the same recipient advisory lock as marketing-consent changes.
#[async_trait::async_trait]
pub trait NewsletterConfirmationChallenges: Send {
    async fn create_if_allowed(
        &mut self,
        challenge: NewNewsletterConfirmationChallenge,
    ) -> Result<NewsletterConfirmationIssueOutcome, NewsletterConfirmationChallengeError>;

    async fn record_send_outcome(
        &mut self,
        id: NewsletterConfirmationId,
        outcome: NewsletterConfirmationSendStatus,
        recorded_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError>;

    async fn find_by_token_digest(
        &mut self,
        digest: NewsletterConfirmationTokenDigest,
    ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>;

    /// Acquires the shared recipient lock, then locks and reloads the exact challenge row.
    async fn lock_for_confirmation(
        &mut self,
        id: NewsletterConfirmationId,
        digest: NewsletterConfirmationTokenDigest,
    ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>;

    async fn invalidate(
        &mut self,
        id: NewsletterConfirmationId,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError>;

    /// Confirms this proof and invalidates every other outstanding challenge for
    /// the exact same mailbox. The caller already holds the recipient lock.
    async fn complete_confirmation(
        &mut self,
        id: NewsletterConfirmationId,
        bound_user_id: Option<UserId>,
        intent_id: MarketingConsentSyncIntentId,
        confirmed_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError>;

    /// Provider withdrawals call this inside their accepted-decision transaction.
    async fn invalidate_pending_for_email(
        &mut self,
        email: &Email,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), NewsletterConfirmationChallengeError>;

    async fn cleanup_expired(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, NewsletterConfirmationChallengeError>;
}

pub trait NewsletterConfirmationChallengesFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl NewsletterConfirmationChallenges + 'tx;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("newsletter confirmation token generation failed")]
pub struct NewsletterConfirmationTokenGenerationError;

pub trait NewsletterConfirmationTokenGenerator: Send + Sync {
    fn generate(
        &self,
    ) -> Result<RawNewsletterConfirmationToken, NewsletterConfirmationTokenGenerationError>;
}

pub trait NewsletterConfirmationClock: Send + Sync {
    fn now_utc(&self) -> OffsetDateTime;
}

#[derive(Clone, Copy, Default)]
pub struct SystemNewsletterConfirmationClock;

impl NewsletterConfirmationClock for SystemNewsletterConfirmationClock {
    fn now_utc(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

#[derive(Clone, Copy, Default)]
pub struct OsNewsletterConfirmationTokenGenerator;

impl NewsletterConfirmationTokenGenerator for OsNewsletterConfirmationTokenGenerator {
    fn generate(
        &self,
    ) -> Result<RawNewsletterConfirmationToken, NewsletterConfirmationTokenGenerationError> {
        let mut entropy = [0u8; 32];
        getrandom::fill(&mut entropy).map_err(|_| NewsletterConfirmationTokenGenerationError)?;
        Ok(RawNewsletterConfirmationToken::from_entropy(entropy))
    }
}
