mod external_template;
mod provider_failure;
mod sender;
mod template_mapping;
mod template_reader;
#[cfg(test)]
mod test_support;

pub use sender::{EmailDeliveryConfig, EmailDeliveryConfigError, SesNotificationChannelSender};
pub use template_reader::EmailTemplateLoadError;
