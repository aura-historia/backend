use crate::ports::notification_recipient_reader::NotificationRecipient;
use application::error::BoxError;
use notification_core::notification::LocalizedNotificationPayload;

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalNotificationMessage {
    pub recipient: NotificationRecipient,
    pub payload: LocalizedNotificationPayload,
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalNotificationSendError {
    #[error("external notification send failed")]
    SendFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait ExternalNotificationSender: Send + Sync {
    async fn send(
        &self,
        message: ExternalNotificationMessage,
    ) -> Result<(), ExternalNotificationSendError>;
}
