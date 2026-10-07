use serde_email::Email;

use super::marketing_consent_intents::ConsentIntent;

/// A provider observation, not a proof of local consent or future deliverability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketingEmailSubscriptionState {
    Missing,
    Present {
        contact_id: String,
        globally_subscribed: bool,
        on_target_list: bool,
        suppressed: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketingEmailConsentOutcome {
    Applied { contact_id: String },
    AlreadyApplied { contact_id: Option<String> },
    BlockedByProviderPreferences,
}

/// Do not include provider responses, credentials or exact email in these errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarketingEmailConsentError {
    #[error("invalid marketing email address")]
    InvalidEmail,
    #[error("ineligible marketing consent intent")]
    IneligibleIntent,
    #[error("marketing consent provider rejected request (status {status:?})")]
    Rejected { status: Option<u16> },
    #[error("marketing consent provider throttled request (status {status:?})")]
    Throttled { status: Option<u16> },
    #[error("marketing consent provider was unreachable before write")]
    NotSent,
    #[error("marketing consent write acceptance unknown")]
    AcceptanceUnknown,
    #[error("marketing consent provider read unavailable")]
    ReadUnavailable,
    #[error("marketing consent provider protocol/config failure (status {status:?})")]
    Protocol { status: Option<u16> },
}

/// Caller must fence and commit an eligible immutable intent before any grant/revoke.
/// This port neither decides consent nor retries writes after ambiguous acceptance.
#[async_trait::async_trait]
pub trait MarketingEmailConsentWriter: Send + Sync {
    async fn current_state(
        &self,
        email: &Email,
    ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError>;

    async fn grant(
        &self,
        intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>;

    async fn revoke(
        &self,
        intent: &ConsentIntent,
    ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>;
}
