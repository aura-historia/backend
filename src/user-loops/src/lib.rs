mod config;
mod marketing_email_consent_writer;
mod newsletter_subscription_writer;
mod newsletter_webhook_verifier;

pub use config::{LoopsNewsletterConfig, LoopsNewsletterConfigError};
pub use marketing_email_consent_writer::LoopsMarketingEmailConsentWriter;
pub use newsletter_subscription_writer::LoopsNewsletterSubscriptionWriter;
pub use newsletter_webhook_verifier::LoopsNewsletterWebhookVerifier;
