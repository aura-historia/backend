use common::{error::boxed::BoxError, notification_id::NotificationId, user_id::UserId};

#[derive(Debug, thiserror::Error)]
pub enum NotificationSeenWriteError {
    #[error("notification seen-state update failed")]
    UpdateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationSeenWriter: Send + Sync {
    async fn set_seen(
        &self,
        user_id: UserId,
        notification_id: NotificationId,
        seen: bool,
    ) -> Result<bool, NotificationSeenWriteError>;

    async fn set_seen_many(
        &self,
        user_id: UserId,
        notification_ids: &[NotificationId],
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError>;

    async fn set_seen_all(
        &self,
        user_id: UserId,
        seen: bool,
    ) -> Result<u64, NotificationSeenWriteError>;
}
