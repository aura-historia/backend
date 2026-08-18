use common::{error::boxed::BoxError, notification_id::NotificationId, user_id::UserId};
use notification_core::notification::NotificationContent;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationListCursor {
    pub created: OffsetDateTime,
    pub notification_id: NotificationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationListItem {
    pub notification_id: NotificationId,
    pub content: NotificationContent,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationListPage {
    pub items: Vec<NotificationListItem>,
    pub next_cursor: Option<NotificationListCursor>,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationListReadError {
    #[error("notification list read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted notification list item is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationListReader: Send + Sync {
    async fn list_for_user(
        &self,
        user_id: UserId,
        cursor: Option<NotificationListCursor>,
        limit: u32,
    ) -> Result<NotificationListPage, NotificationListReadError>;
}
