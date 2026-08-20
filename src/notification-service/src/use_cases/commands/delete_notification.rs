use crate::ports::{
    notification_deleter::{NotificationDeleteError, NotificationDeleter},
    notification_repository::{NotificationRepository, NotificationRepositoryError},
};
use domain_primitives::event_id::EventId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeleteNotificationCommand {
    pub user_id: UserId,
    pub origin_event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteNotificationError {
    #[error("notification not found")]
    NotFound,
    #[error("notification lookup failed")]
    LookupFailed(#[source] NotificationRepositoryError),
    #[error("notification delete failed")]
    DeleteFailed(#[source] NotificationDeleteError),
}

#[async_trait::async_trait]
pub trait DeleteNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: DeleteNotificationCommand,
    ) -> Result<(), DeleteNotificationError>;
}

pub struct DeleteNotificationHandler<R, D> {
    repository: R,
    deleter: D,
}

impl<R, D> DeleteNotificationHandler<R, D> {
    pub fn new(repository: R, deleter: D) -> Self {
        Self {
            repository,
            deleter,
        }
    }
}

#[async_trait::async_trait]
impl<R, D> DeleteNotificationUseCase for DeleteNotificationHandler<R, D>
where
    R: NotificationRepository,
    D: NotificationDeleter,
{
    async fn execute(
        &self,
        command: DeleteNotificationCommand,
    ) -> Result<(), DeleteNotificationError> {
        self.repository
            .find_by_origin_event_id(&command.user_id, &command.origin_event_id)
            .await
            .map_err(DeleteNotificationError::LookupFailed)?
            .ok_or(DeleteNotificationError::NotFound)?;
        self.deleter
            .delete_by_origin_event_id(&command.user_id, &command.origin_event_id)
            .await
            .map_err(DeleteNotificationError::DeleteFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::{BoxError, box_error};
    use notification_core::notification::{
        Notification, NotificationPartnerApplicationPayload, NotificationPayload,
    };
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeGateway {
        notification: Arc<Mutex<Option<Notification>>>,
        deleted: Arc<Mutex<Vec<EventId>>>,
    }

    fn notification_for(user_id: UserId, origin_event_id: EventId) -> Notification {
        Notification::new(
            user_id,
            origin_event_id,
            NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("test shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id: PartnerShopApplicationId::new(),
                },
            },
            true,
        )
    }

    #[async_trait::async_trait]
    impl NotificationRepository for FakeGateway {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }

        async fn find_by_origin_event_id(
            &self,
            user_id: &UserId,
            origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            Ok(self
                .notification
                .lock()
                .unwrap()
                .clone()
                .filter(|notification| {
                    notification.user_id() == *user_id
                        && notification.origin_event_id() == *origin_event_id
                }))
        }

        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }
    }

    #[async_trait::async_trait]
    impl NotificationDeleter for FakeGateway {
        async fn delete_by_origin_event_id(
            &self,
            _user_id: &UserId,
            origin_event_id: &EventId,
        ) -> Result<(), NotificationDeleteError> {
            self.deleted.lock().unwrap().push(*origin_event_id);
            Ok(())
        }

        async fn delete_many_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_ids: &[EventId],
        ) -> Result<(), NotificationDeleteError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_delete_notification_when_it_exists() {
        let gateway = FakeGateway::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *gateway.notification.lock().unwrap() = Some(notification_for(user_id, origin_event_id));

        DeleteNotificationHandler::new(gateway.clone(), gateway.clone())
            .execute(DeleteNotificationCommand {
                user_id,
                origin_event_id,
            })
            .await
            .expect("delete should succeed");

        assert_eq!(vec![origin_event_id], *gateway.deleted.lock().unwrap());
    }

    #[tokio::test]
    async fn should_return_not_found_when_delete_notification_missing() {
        let gateway = FakeGateway::default();

        let result = DeleteNotificationHandler::new(gateway.clone(), gateway)
            .execute(DeleteNotificationCommand {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
            })
            .await;

        assert!(matches!(result, Err(DeleteNotificationError::NotFound)));
    }

    #[derive(Clone, Default)]
    struct FailingDeleter;

    #[async_trait::async_trait]
    impl NotificationDeleter for FailingDeleter {
        async fn delete_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_id: &EventId,
        ) -> Result<(), NotificationDeleteError> {
            let source: BoxError = box_error(std::io::Error::other("boom"));
            Err(NotificationDeleteError::OperationFailed { source })
        }

        async fn delete_many_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_ids: &[EventId],
        ) -> Result<(), NotificationDeleteError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_fail_delete_notification_when_delete_fails() {
        let gateway = FakeGateway::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *gateway.notification.lock().unwrap() = Some(notification_for(user_id, origin_event_id));

        let result = DeleteNotificationHandler::new(gateway, FailingDeleter)
            .execute(DeleteNotificationCommand {
                user_id,
                origin_event_id,
            })
            .await;

        assert!(matches!(
            result,
            Err(DeleteNotificationError::DeleteFailed(_))
        ));
    }
}
