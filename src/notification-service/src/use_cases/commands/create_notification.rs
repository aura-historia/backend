use crate::ports::{
    NotificationWriteOutcome, NotificationWriter, notification_writer::NotificationWriteError,
};
use domain_primitives::event_id::EventId;
use notification_core::notification::{Notification, NotificationPayload};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationCommand {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_payload: NotificationPayload,
    pub external: bool,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum CreateNotificationResult {
    Created { notification: Notification },
    AlreadyExists,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateNotificationError {
    #[error("notification insert failed")]
    InsertFailed(#[source] NotificationWriteError),
}

#[async_trait::async_trait]
pub trait CreateNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: CreateNotificationCommand,
    ) -> Result<CreateNotificationResult, CreateNotificationError>;
}

pub struct CreateNotificationHandler<R> {
    repository: R,
}

impl<R> CreateNotificationHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<R> CreateNotificationUseCase for CreateNotificationHandler<R>
where
    R: NotificationWriter,
{
    async fn execute(
        &self,
        command: CreateNotificationCommand,
    ) -> Result<CreateNotificationResult, CreateNotificationError> {
        let notification = Notification::new(
            command.user_id,
            command.origin_event_id,
            command.notification_payload,
            command.external,
        );
        match self
            .repository
            .insert(&notification)
            .await
            .map_err(CreateNotificationError::InsertFailed)?
        {
            NotificationWriteOutcome::Inserted(notification) => {
                Ok(CreateNotificationResult::Created { notification })
            }
            NotificationWriteOutcome::AlreadyExists => Ok(CreateNotificationResult::AlreadyExists),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::{BoxError, box_error};
    use notification_core::notification::NotificationPartnerApplicationPayload;
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy)]
    enum FakeWriteOutcome {
        Inserted,
        AlreadyExists,
        Fails,
    }

    #[derive(Clone)]
    struct FakeWriter {
        notifications: Arc<Mutex<Vec<Notification>>>,
        outcome: FakeWriteOutcome,
    }

    impl Default for FakeWriter {
        fn default() -> Self {
            Self {
                notifications: Arc::default(),
                outcome: FakeWriteOutcome::Inserted,
            }
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn payload() -> NotificationPayload {
        NotificationPayload::PartnerApplication {
            shop_name: ShopName::from("test shop"),
            image: None,
            partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                partner_application_id: PartnerShopApplicationId::new(),
            },
        }
    }

    #[async_trait::async_trait]
    impl NotificationWriter for FakeWriter {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<NotificationWriteOutcome, NotificationWriteError> {
            match self.outcome {
                FakeWriteOutcome::Inserted => {
                    self.notifications
                        .lock()
                        .map_err(|_| NotificationWriteError::WriteFailed { source: boxed() })?
                        .push(notification.clone());
                    Ok(NotificationWriteOutcome::Inserted(notification.clone()))
                }
                FakeWriteOutcome::AlreadyExists => Ok(NotificationWriteOutcome::AlreadyExists),
                FakeWriteOutcome::Fails => {
                    Err(NotificationWriteError::WriteFailed { source: boxed() })
                }
            }
        }
    }

    #[tokio::test]
    async fn should_create_notification_when_insert_succeeds() {
        let repository = FakeWriter::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();

        let result = CreateNotificationHandler::new(repository.clone())
            .execute(CreateNotificationCommand {
                user_id,
                origin_event_id,
                notification_payload: payload(),
                external: true,
            })
            .await
            .expect("create should succeed");

        assert!(matches!(
            result,
            CreateNotificationResult::Created { notification } if notification.user_id() == user_id
        ));
        assert_eq!(1, repository.notifications.lock().unwrap().len());
    }

    #[tokio::test]
    async fn should_fail_create_notification_when_insert_fails() {
        let repository = FakeWriter {
            notifications: Arc::default(),
            outcome: FakeWriteOutcome::Fails,
        };

        let result = CreateNotificationHandler::new(repository)
            .execute(CreateNotificationCommand {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_payload: payload(),
                external: true,
            })
            .await;

        assert!(matches!(
            result,
            Err(CreateNotificationError::InsertFailed(_))
        ));
    }

    #[tokio::test]
    async fn should_report_already_exists_without_returning_a_notification() {
        let writer = FakeWriter {
            notifications: Arc::default(),
            outcome: FakeWriteOutcome::AlreadyExists,
        };

        let result = CreateNotificationHandler::new(writer)
            .execute(CreateNotificationCommand {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                notification_payload: payload(),
                external: true,
            })
            .await;

        assert!(matches!(
            result,
            Ok(CreateNotificationResult::AlreadyExists)
        ));
    }
}
