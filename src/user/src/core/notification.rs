use crate::core::notification_id::NotificationId;
use common::user_id::UserId;
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct Notification {
    pub user_id: UserId,
    pub notification_id: NotificationId,
    pub seen: bool,
    pub payload: NotificationPayload,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationPayload {
    Watchlist(),
}
