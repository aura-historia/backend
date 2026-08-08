use crate::ports::notification_repository::{NotificationRepository, NotificationRepositoryError};
use notification_core::notification::Notification;

#[async_trait::async_trait]
pub trait NotificationWriter: Send + Sync {
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError>;
}

#[async_trait::async_trait]
impl<T> NotificationWriter for T
where
    T: NotificationRepository,
{
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError> {
        NotificationRepository::insert(self, notification).await
    }
}
