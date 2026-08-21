use crate::ports::notification_creator::{
    NewNotification, NotificationCreationError, NotificationCreationOutcome,
};

#[async_trait::async_trait]
pub trait NotificationRepository: Send {
    async fn insert_many(
        &mut self,
        notifications: &[NewNotification],
    ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError>;
}

pub trait NotificationRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl NotificationRepository + 'tx;
}
