use crate::ports::notification_repository::{NotificationRepository, NotificationRepositoryError};
use common::{event_id::EventId, user_id::UserId};
use notification_core::notification::Notification;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindNotificationRequest {
    pub user_id: UserId,
    pub origin_event_id: EventId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FindNotificationResult {
    pub notification: Notification,
}

#[derive(Debug, thiserror::Error)]
pub enum FindNotificationError {
    #[error("notification not found")]
    NotFound,
    #[error("notification lookup failed")]
    LookupFailed(#[source] NotificationRepositoryError),
}

#[async_trait::async_trait]
pub trait FindNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        request: FindNotificationRequest,
    ) -> Result<FindNotificationResult, FindNotificationError>;
}

pub struct FindNotificationHandler<R> {
    repository: R,
}

impl<R> FindNotificationHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<R> FindNotificationUseCase for FindNotificationHandler<R>
where
    R: NotificationRepository,
{
    async fn execute(
        &self,
        request: FindNotificationRequest,
    ) -> Result<FindNotificationResult, FindNotificationError> {
        let notification = self
            .repository
            .find_by_origin_event_id(&request.user_id, &request.origin_event_id)
            .await
            .map_err(FindNotificationError::LookupFailed)?
            .ok_or(FindNotificationError::NotFound)?;
        Ok(FindNotificationResult { notification })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_name::ShopName};
    use notification_core::notification::{
        NotificationPartnerApplicationPayload, NotificationPayload,
    };

    #[derive(Clone)]
    struct FakeRepository(Option<Notification>);

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
    impl NotificationRepository for FakeRepository {
        async fn insert(
            &self,
            _notification: &Notification,
        ) -> Result<(), NotificationRepositoryError> {
            Ok(())
        }

        async fn find_by_origin_event_id(
            &self,
            user_id: &UserId,
            origin_event_id: &EventId,
        ) -> Result<Option<Notification>, NotificationRepositoryError> {
            Ok(self.0.clone().filter(|notification| {
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

    #[tokio::test]
    async fn should_find_notification_when_it_exists() {
        let user_id = UserId::new();
        let origin_event_id = EventId::new();
        let request = FindNotificationRequest {
            user_id,
            origin_event_id,
        };

        let result = FindNotificationHandler::new(FakeRepository(Some(notification_for(
            user_id,
            origin_event_id,
        ))))
        .execute(request)
        .await
        .expect("find should succeed");

        assert_eq!(origin_event_id, result.notification.origin_event_id());
    }

    #[tokio::test]
    async fn should_return_not_found_when_find_notification_missing() {
        let request = FindNotificationRequest {
            user_id: UserId::new(),
            origin_event_id: EventId::new(),
        };

        let result = FindNotificationHandler::new(FakeRepository(None))
            .execute(request)
            .await;

        assert!(matches!(result, Err(FindNotificationError::NotFound)));
    }
}
