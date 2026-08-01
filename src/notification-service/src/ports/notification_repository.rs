use common::{error::boxed::BoxError, event_id::EventId, user_id::UserId};
use notification_core::notification::Notification;

#[derive(Debug, thiserror::Error)]
pub enum NotificationRepositoryError {
    #[error("notification persistence operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted notification state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationRepository: Send + Sync {
    async fn insert(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError>;

    async fn find_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<Option<Notification>, NotificationRepositoryError>;

    async fn update(
        &self,
        notification: &Notification,
    ) -> Result<Notification, NotificationRepositoryError>;
}
