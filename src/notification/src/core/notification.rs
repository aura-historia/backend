use crate::core::{
    notification_id::NotificationId, notification_reason::NotificationReason,
    notification_type::NotificationType,
};
use common::user_id::UserId;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct Notification {
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub notification_type: NotificationType,
    pub notification_reason: NotificationReason,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
