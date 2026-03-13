use crate::core::notification::NotificationPayload;
use common::user_id::UserId;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone)]
pub struct CreateNotificationCommand {
    pub user_id: UserId,
    pub notification_payload: NotificationPayload,
    pub external: bool,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, Default)]
pub struct UpdateNotificationCommand {
    pub seen: Option<bool>,
}

impl UpdateNotificationCommand {
    pub fn is_empty(&self) -> bool {
        self.seen.is_none()
    }
}
