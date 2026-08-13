use common::error::boxed::BoxError;
use user_core::newsletter_subscription::NewsletterSubscription;

#[derive(Debug, thiserror::Error)]
pub enum NewsletterSubscriptionWriteError {
    #[error("invalid newsletter subscription email")]
    InvalidEmail,
    #[error("newsletter subscription service temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("internal newsletter subscription write failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NewsletterSubscriptionWriter: Send + Sync {
    async fn upsert(
        &self,
        subscription: &NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError>;
}
