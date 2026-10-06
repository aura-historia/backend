use crate::ports::NewsletterProfile;
use serde_email::Email;
use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;
use user_core::newsletter_confirmation_id::NewsletterConfirmationId;

/// Transient sender input; it contains the raw proof only while constructing the link.
/// Deliberately has no Debug implementation and is never persisted.
pub struct NewsletterConfirmationEmail {
    pub confirmation_id: NewsletterConfirmationId,
    pub recipient: Email,
    pub token: RawNewsletterConfirmationToken,
    pub profile: NewsletterProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewsletterConfirmationEmailSendOutcome {
    Accepted,
    DefinitelyRejected,
    AcceptanceUnknown,
}

/// Adapter errors are returned only as these safe categories; no raw provider error is kept.
#[async_trait::async_trait]
pub trait NewsletterConfirmationEmailSender: Send + Sync {
    async fn send(
        &self,
        email: NewsletterConfirmationEmail,
    ) -> NewsletterConfirmationEmailSendOutcome;
}
