use application::error::BoxError;
use domain_primitives::event_id::EventId;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeleteError {
    #[error("notification delete failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationDeleter: Send + Sync {
    async fn delete_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_id: &EventId,
    ) -> Result<(), NotificationDeleteError>;

    async fn delete_many_by_origin_event_id(
        &self,
        user_id: &UserId,
        origin_event_ids: &[EventId],
    ) -> Result<(), NotificationDeleteError>;
}
