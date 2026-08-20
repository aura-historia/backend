use application::error::BoxError;
use notification_core::notification::Notification;

#[derive(Debug, thiserror::Error)]
pub enum NotificationBatchInsertError {
    #[error("notification batch insert failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationBatchInserter: Send + Sync {
    async fn insert_many(
        &self,
        notifications: &[Notification],
    ) -> Result<Vec<Notification>, NotificationBatchInsertError>;
}
