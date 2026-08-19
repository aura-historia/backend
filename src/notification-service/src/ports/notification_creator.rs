use common::{error::boxed::BoxError, notification_id::NotificationId};
use notification_core::notification::Notification;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalDeliveryRequest {
    None,
    Requested,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewNotification {
    pub notification: Notification,
    pub external_delivery: ExternalDeliveryRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationCreationOutcome {
    Inserted { notification_id: NotificationId },
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationCreationError {
    #[error("notification creation failed")]
    CreateFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationCreator: Send {
    async fn create_many(
        &mut self,
        notifications: &[NewNotification],
    ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError>;
}

pub trait NotificationCreatorFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl NotificationCreator + 'tx;
}
