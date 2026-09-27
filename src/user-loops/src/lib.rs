mod config;
mod newsletter_subscription_writer;

pub use config::{LoopsNewsletterConfig, LoopsNewsletterConfigError};
pub use newsletter_subscription_writer::LoopsNewsletterSubscriptionWriter;
