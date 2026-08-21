use crate::ports::notification_repository::{NotificationRepository, NotificationRepositoryError};
use domain_primitives::event_id::EventId;
use notification_core::notification::Notification;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UpdateNotificationCommand {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub seen: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationResult {
    pub notification: Notification,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateNotificationError {
    #[error("notification not found")]
    NotFound,
    #[error("notification lookup failed")]
    LookupFailed(#[source] NotificationRepositoryError),
    #[error("notification update failed")]
    UpdateFailed(#[source] NotificationRepositoryError),
}

#[async_trait::async_trait]
pub trait UpdateNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: UpdateNotificationCommand,
    ) -> Result<UpdateNotificationResult, UpdateNotificationError>;
}

pub struct UpdateNotificationHandler<R> {
    repository: R,
}

impl<R> UpdateNotificationHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<R> UpdateNotificationUseCase for UpdateNotificationHandler<R>
where
    R: NotificationRepository,
{
    async fn execute(
        &self,
        command: UpdateNotificationCommand,
    ) -> Result<UpdateNotificationResult, UpdateNotificationError> {
        let mut notification = self
            .repository
            .find_by_origin_event_id(&command.user_id, &command.origin_event_id)
            .await
            .map_err(UpdateNotificationError::LookupFailed)?
            .ok_or(UpdateNotificationError::NotFound)?;

        if let Some(seen) = command.seen {
            notification.mark_seen(seen);
            notification = self
                .repository
                .update(&notification)
                .await
                .map_err(UpdateNotificationError::UpdateFailed)?;
        }

        Ok(UpdateNotificationResult { notification })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::{BoxError, box_error};
    use notification_core::{
        notification::{
            NotificationPartnerApplicationPayload, NotificationPayload, RehydratedNotificationState,
        },
        notification_id::NotificationId,
    };
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeRepository {
        notification: Arc<Mutex<Option<Notification>>>,
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

    fn rehydrated_notification(
        user_id: UserId,
        origin_event_id: EventId,
        seen: bool,
    ) -> Notification {
        Notification::rehydrate(RehydratedNotificationState {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: payload(),
            seen,
            external: true,
        })
    }

    #[async_trait::async_trait]
    impl NotificationRepository for FakeRepository {
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
            *self.notification.lock().unwrap() = Some(notification.clone());
            Ok(notification.clone())
        }
    }

    #[tokio::test]
    async fn should_update_notification_seen_when_notification_exists() {
        let repository = FakeRepository::default();
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        *repository.notification.lock().unwrap() =
            Some(rehydrated_notification(user_id, origin_event_id, false));

        let result = UpdateNotificationHandler::new(repository)
            .execute(UpdateNotificationCommand {
                user_id,
                origin_event_id,
                seen: Some(true),
            })
            .await
            .expect("update should succeed");

        assert!(result.notification.seen());
    }

    #[tokio::test]
    async fn should_return_not_found_when_update_notification_missing() {
        let result = UpdateNotificationHandler::new(FakeRepository::default())
            .execute(UpdateNotificationCommand {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                seen: Some(true),
            })
            .await;

        assert!(matches!(result, Err(UpdateNotificationError::NotFound)));
    }

    #[derive(Clone, Default)]
    struct FailingRepository;

    #[async_trait::async_trait]
    impl NotificationRepository for FailingRepository {
        async fn insert(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }

        async fn find_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            let source: BoxError = box_error(std::io::Error::other("boom"));
            Err(NotificationRepositoryError::OperationFailed { source })
        }

        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            Ok(notification.clone())
        }
    }

    #[tokio::test]
    async fn should_fail_update_notification_when_lookup_fails() {
        let result = UpdateNotificationHandler::new(FailingRepository)
            .execute(UpdateNotificationCommand {
                user_id: UserId::new(),
                origin_event_id: EventId::new(),
                seen: Some(true),
            })
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationError::LookupFailed(_))
        ));
    }
}
