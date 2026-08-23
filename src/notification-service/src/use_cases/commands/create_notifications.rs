use crate::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError,
    NotificationCreationOutcome, NotificationCreator, NotificationCreatorFactory,
};
use application::{
    error::box_error,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use notification_core::{
    notification::{Notification, NotificationContent},
    notification_id::NotificationId,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationIntent {
    pub notification_id: NotificationId,
    pub user_id: UserId,
    pub content: NotificationContent,
    pub external_delivery: ExternalDeliveryRequest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationsCommand {
    pub intents: Vec<CreateNotificationIntent>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationsResult {
    pub outcomes: Vec<NotificationCreationOutcome>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateNotificationsError {
    #[error("notification transaction failed")]
    TransactionFailed(#[from] TransactionError),
    #[error("notification creation failed")]
    CreateFailed(#[from] NotificationCreationError),
}

#[async_trait::async_trait]
pub trait CreateNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        command: CreateNotificationsCommand,
    ) -> Result<CreateNotificationsResult, CreateNotificationsError>;
}

pub struct CreateNotificationsHandler<U, C> {
    unit_of_work: U,
    creator: C,
}

impl<U, C> CreateNotificationsHandler<U, C> {
    pub fn new(unit_of_work: U, creator: C) -> Self {
        Self {
            unit_of_work,
            creator,
        }
    }
}

#[async_trait::async_trait]
impl<U, C> CreateNotificationsUseCase for CreateNotificationsHandler<U, C>
where
    U: UnitOfWork,
    C: NotificationCreatorFactory<U::Tx>,
{
    async fn execute(
        &self,
        command: CreateNotificationsCommand,
    ) -> Result<CreateNotificationsResult, CreateNotificationsError> {
        let notifications = command
            .intents
            .into_iter()
            .map(|intent| NewNotification {
                notification: Notification::new(
                    intent.notification_id,
                    intent.user_id,
                    intent.content,
                ),
                external_delivery: intent.external_delivery,
            })
            .collect::<Vec<_>>();
        let mut tx = self.unit_of_work.begin().await?;
        let outcomes = self
            .creator
            .in_transaction(&mut tx)
            .create_many(&notifications)
            .await?;
        if outcomes.len() != notifications.len() {
            return Err(CreateNotificationsError::CreateFailed(
                NotificationCreationError::CreateFailed {
                    source: box_error(std::io::Error::other(
                        "notification creator returned incomplete outcomes",
                    )),
                },
            ));
        }
        tx.commit().await?;
        Ok(CreateNotificationsResult { outcomes })
    }
}
