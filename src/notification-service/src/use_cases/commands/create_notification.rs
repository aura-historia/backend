use crate::ports::notification_repository::{NotificationRepository, NotificationRepositoryError};
use common::{event_id::EventId, user_id::UserId};
use notification_core::notification::{Notification, NotificationPayload};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationCommand {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_payload: NotificationPayload,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationResult {
    pub notification: Notification,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateNotificationError {
    #[error("notification insert failed")]
    InsertFailed(#[source] NotificationRepositoryError),
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
    R: NotificationRepository,
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
        self.repository
            .insert(&notification)
            .await
            .map_err(CreateNotificationError::InsertFailed)?;
        Ok(CreateNotificationResult { notification })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{
        error::boxed::{BoxError, box_error},
        partner_shop_application_id::PartnerShopApplicationId,
        shop_name::ShopName,
    };
    use notification_core::notification::NotificationPartnerApplicationPayload;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeRepository {
        notifications: Arc<Mutex<Vec<Notification>>>,
        fail: bool,
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
    impl NotificationRepository for FakeRepository {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<(), NotificationRepositoryError> {
            if self.fail {
                return Err(NotificationRepositoryError::OperationFailed { source: boxed() });
            }
            self.notifications
                .lock()
                .unwrap()
                .push(notification.clone());
            Ok(())
        }

        async fn find_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            Ok(None)
        }

        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }
    }

    #[tokio::test]
    async fn should_create_notification_when_insert_succeeds() {
        let repository = FakeRepository::default();
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

        assert_eq!(user_id, result.notification.user_id());
        assert_eq!(1, repository.notifications.lock().unwrap().len());
    }

    #[tokio::test]
    async fn should_fail_create_notification_when_insert_fails() {
        let repository = FakeRepository {
            fail: true,
            ..Default::default()
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
}
