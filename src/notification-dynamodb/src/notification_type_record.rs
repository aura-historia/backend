use notification_core::notification_type::NotificationType;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationTypeRecord {
    Email,
}

impl From<NotificationTypeRecord> for NotificationType {
    fn from(value: NotificationTypeRecord) -> NotificationType {
        match value {
            NotificationTypeRecord::Email => NotificationType::Email,
        }
    }
}

impl From<NotificationType> for NotificationTypeRecord {
    fn from(value: NotificationType) -> NotificationTypeRecord {
        match value {
            NotificationType::Email => NotificationTypeRecord::Email,
        }
    }
}
