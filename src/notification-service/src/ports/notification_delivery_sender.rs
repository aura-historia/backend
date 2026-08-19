use crate::ports::notification_delivery_repository::NotificationDeliverySource;
use common::error::boxed::BoxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentNotificationDelivery {
    pub provider_message_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeliverySendError {
    #[error("notification delivery send failed temporarily: {code}")]
    Retryable {
        code: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("notification delivery send failed permanently: {code}")]
    Permanent {
        code: &'static str,
        #[source]
        source: BoxError,
    },
}

impl NotificationDeliverySendError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Retryable { code, .. } | Self::Permanent { code, .. } => code,
        }
    }
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationDeliverySender: Send + Sync {
    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationDeliverySendError>;
}
