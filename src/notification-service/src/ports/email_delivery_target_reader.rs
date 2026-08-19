use common::{error::boxed::BoxError, user_id::UserId};
use notification_core::notification_delivery::NotificationDeliveryTargetKey;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct EmailDeliveryTarget {
    pub address: Email,
    pub first_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EmailDeliveryTargetReadError {
    #[error("email delivery target lookup failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted email delivery target is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait EmailDeliveryTargetReader: Send + Sync {
    async fn find_email_target(
        &self,
        user_id: UserId,
        target_key: &NotificationDeliveryTargetKey,
    ) -> Result<Option<EmailDeliveryTarget>, EmailDeliveryTargetReadError>;
}
