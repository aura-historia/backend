use crate::ports::notification_repository::NotificationRepository;
use application::error::{BoxError, box_error};
use notification_core::notification::Notification;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationWriteOutcome {
    Inserted(Notification),
    AlreadyExists,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationWriteError {
    #[error("notification write failed")]
    WriteFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationWriter: Send + Sync {
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<NotificationWriteOutcome, NotificationWriteError>;
}

#[async_trait::async_trait]
impl<T> NotificationWriter for T
where
    T: NotificationRepository,
{
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<NotificationWriteOutcome, NotificationWriteError> {
        NotificationRepository::insert(self, notification)
            .await
            .map(NotificationWriteOutcome::Inserted)
            .map_err(|source| NotificationWriteError::WriteFailed {
                source: box_error(source),
            })
    }
}
