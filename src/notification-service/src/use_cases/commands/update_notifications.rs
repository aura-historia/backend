use crate::ports::{
    all_notifications_reader::{AllNotificationsReadError, AllNotificationsReader},
    notification_repository::{NotificationRepository, NotificationRepositoryError},
};
use notification_core::notification::{Notification, RehydratedNotificationState};
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UpdateNotificationsCommand {
    pub user_id: UserId,
    pub seen: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateNotificationsResult {
    pub notifications: Vec<Notification>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateNotificationsError {
    #[error("notification list failed")]
    ReadFailed(#[source] AllNotificationsReadError),
    #[error("notification update failed")]
    UpdateFailed(#[source] NotificationRepositoryError),
}

#[async_trait::async_trait]
pub trait UpdateNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        command: UpdateNotificationsCommand,
    ) -> Result<UpdateNotificationsResult, UpdateNotificationsError>;
}

pub struct UpdateNotificationsHandler<R, W> {
    reader: R,
    repository: W,
}

impl<R, W> UpdateNotificationsHandler<R, W> {
    pub fn new(reader: R, repository: W) -> Self {
        Self { reader, repository }
    }
}

#[async_trait::async_trait]
impl<R, W> UpdateNotificationsUseCase for UpdateNotificationsHandler<R, W>
where
    R: AllNotificationsReader,
    W: NotificationRepository,
{
    async fn execute(
        &self,
        command: UpdateNotificationsCommand,
    ) -> Result<UpdateNotificationsResult, UpdateNotificationsError> {
        let Some(seen) = command.seen else {
            return Ok(UpdateNotificationsResult {
                notifications: Vec::new(),
            });
        };

        let items = self
            .reader
            .list_all_by_user(&command.user_id)
            .await
            .map_err(UpdateNotificationsError::ReadFailed)?;
        let mut notifications = Vec::with_capacity(items.len());
        for item in items {
            let mut notification = Notification::rehydrate(RehydratedNotificationState {
                user_id: item.user_id,
                origin_event_id: item.origin_event_id,
                notification_id: item.notification_id,
                notification_type: item.notification_type,
                notification_payload: item.notification_payload,
                seen: item.seen,
                external: item.external,
            });
            notification.mark_seen(seen);
            notifications.push(
                self.repository
                    .update(&notification)
                    .await
                    .map_err(UpdateNotificationsError::UpdateFailed)?,
            );
        }

        Ok(UpdateNotificationsResult { notifications })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::all_notifications_reader::AllNotificationsReadItem;
    use application::error::{BoxError, box_error};
    use domain_primitives::event_id::EventId;
    use notification_core::{
        notification::{NotificationPartnerApplicationPayload, NotificationPayload},
        notification_id::NotificationId,
    };
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

    #[derive(Clone, Default)]
    struct FakeReaderWriter {
        items: Arc<Mutex<Vec<AllNotificationsReadItem>>>,
        updated: Arc<Mutex<Vec<Notification>>>,
    }

    fn item(user_id: UserId, seen: bool) -> AllNotificationsReadItem {
        AllNotificationsReadItem {
            user_id,
            origin_event_id: EventId::new(),
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("test shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id: PartnerShopApplicationId::new(),
                },
            },
            seen,
            external: true,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[async_trait::async_trait]
    impl AllNotificationsReader for FakeReaderWriter {
        async fn list_all_by_user(
            &self,
            user_id: &UserId,
        ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
            Ok(self
                .items
                .lock()
                .unwrap()
                .iter()
                .filter(|item| item.user_id == *user_id)
                .cloned()
                .collect())
        }
    }

    #[async_trait::async_trait]
    impl NotificationRepository for FakeReaderWriter {
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
            Ok(None)
        }

        async fn update(
            &self,
            notification: &Notification,
        ) -> Result<Notification, NotificationRepositoryError> {
            self.updated.lock().unwrap().push(notification.clone());
            Ok(notification.clone())
        }
    }

    #[tokio::test]
    async fn should_update_all_notifications_when_seen_provided() {
        let gateway = FakeReaderWriter::default();
        let user_id = UserId::new();
        *gateway.items.lock().unwrap() = vec![item(user_id, false), item(user_id, false)];

        let result = UpdateNotificationsHandler::new(gateway.clone(), gateway.clone())
            .execute(UpdateNotificationsCommand {
                user_id,
                seen: Some(true),
            })
            .await
            .expect("update all should succeed");

        assert_eq!(2, result.notifications.len());
        assert!(result.notifications.iter().all(Notification::seen));
        assert_eq!(2, gateway.updated.lock().unwrap().len());
    }

    #[tokio::test]
    async fn should_skip_update_all_notifications_when_seen_absent() {
        let gateway = FakeReaderWriter::default();

        let result = UpdateNotificationsHandler::new(gateway.clone(), gateway.clone())
            .execute(UpdateNotificationsCommand {
                user_id: UserId::new(),
                seen: None,
            })
            .await
            .expect("update all should succeed");

        assert!(result.notifications.is_empty());
        assert!(gateway.updated.lock().unwrap().is_empty());
    }

    #[derive(Clone, Default)]
    struct FailingReader;

    #[async_trait::async_trait]
    impl AllNotificationsReader for FailingReader {
        async fn list_all_by_user(
            &self,
            _user_id: &UserId,
        ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError> {
            let source: BoxError = box_error(std::io::Error::other("boom"));
            Err(AllNotificationsReadError::OperationFailed { source })
        }
    }

    #[tokio::test]
    async fn should_fail_update_all_notifications_when_read_fails() {
        let result = UpdateNotificationsHandler::new(FailingReader, FakeReaderWriter::default())
            .execute(UpdateNotificationsCommand {
                user_id: UserId::new(),
                seen: Some(true),
            })
            .await;

        assert!(matches!(
            result,
            Err(UpdateNotificationsError::ReadFailed(_))
        ));
    }
}
