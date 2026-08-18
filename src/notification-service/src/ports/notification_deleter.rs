use common::{error::boxed::BoxError, notification_id::NotificationId, user_id::UserId};

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeleteError {
    #[error("notification delete failed")]
    DeleteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationDeleter: Send + Sync {
    async fn delete_one(
        &self,
        user_id: UserId,
        notification_id: NotificationId,
    ) -> Result<bool, NotificationDeleteError>;

    async fn delete_all(&self, user_id: UserId) -> Result<u64, NotificationDeleteError>;
}
