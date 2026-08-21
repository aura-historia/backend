use crate::ports::notification_batch_inserter::{
    NotificationBatchInsertError, NotificationBatchInserter,
};
use domain_primitives::event_id::EventId;
use notification_core::notification::{Notification, NotificationPayload};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationItem {
    pub user_id: UserId,
    pub notification_payload: NotificationPayload,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationsCommand {
    pub origin_event_id: EventId,
    pub items: Vec<CreateNotificationItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateNotificationsResult {
    pub notifications: Vec<Notification>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateNotificationsError {
    #[error("notification batch insert failed")]
    InsertFailed(#[source] NotificationBatchInsertError),
}

#[async_trait::async_trait]
pub trait CreateNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        command: CreateNotificationsCommand,
    ) -> Result<CreateNotificationsResult, CreateNotificationsError>;
}

pub struct CreateNotificationsHandler<B> {
    batch_inserter: B,
}

impl<B> CreateNotificationsHandler<B> {
    pub fn new(batch_inserter: B) -> Self {
        Self { batch_inserter }
    }
}

#[async_trait::async_trait]
impl<B> CreateNotificationsUseCase for CreateNotificationsHandler<B>
where
    B: NotificationBatchInserter,
{
    async fn execute(
        &self,
        command: CreateNotificationsCommand,
    ) -> Result<CreateNotificationsResult, CreateNotificationsError> {
        let notifications = command
            .items
            .into_iter()
            .map(|item| {
                Notification::new(
                    item.user_id,
                    command.origin_event_id,
                    item.notification_payload,
                    item.external,
                )
            })
            .collect::<Vec<_>>();
        let notifications = self
            .batch_inserter
            .insert_many(&notifications)
            .await
            .map_err(CreateNotificationsError::InsertFailed)?;
        Ok(CreateNotificationsResult { notifications })
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

    #[derive(Clone, Default)]
    struct FakeBatchInserter {
        notifications: Arc<Mutex<Vec<Notification>>>,
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
    impl NotificationBatchInserter for FakeBatchInserter {
        async fn insert_many(
            &self,
            notifications: &[Notification],
        ) -> Result<Vec<Notification>, NotificationBatchInsertError> {
            self.notifications
                .lock()
                .unwrap()
                .extend_from_slice(notifications);
            Ok(notifications.to_vec())
        }
    }

    #[tokio::test]
    async fn should_create_notifications_when_batch_insert_succeeds() {
        let inserter = FakeBatchInserter::default();

        let result = CreateNotificationsHandler::new(inserter.clone())
            .execute(CreateNotificationsCommand {
                origin_event_id: EventId::new(),
                items: vec![
                    CreateNotificationItem {
                        user_id: UserId::new(),
                        notification_payload: payload(),
                        external: true,
                    },
                    CreateNotificationItem {
                        user_id: UserId::new(),
                        notification_payload: payload(),
                        external: false,
                    },
                ],
            })
            .await
            .expect("batch create should succeed");

        assert_eq!(2, result.notifications.len());
        assert_eq!(2, inserter.notifications.lock().unwrap().len());
    }

    #[derive(Clone, Default)]
    struct FailingBatchInserter;

    #[async_trait::async_trait]
    impl NotificationBatchInserter for FailingBatchInserter {
        async fn insert_many(
            &self,
            _notifications: &[Notification],
        ) -> Result<Vec<Notification>, NotificationBatchInsertError> {
            let source: BoxError = box_error(std::io::Error::other("boom"));
            Err(NotificationBatchInsertError::OperationFailed { source })
        }
    }

    #[tokio::test]
    async fn should_fail_create_notifications_when_batch_insert_fails() {
        let result = CreateNotificationsHandler::new(FailingBatchInserter)
            .execute(CreateNotificationsCommand {
                origin_event_id: EventId::new(),
                items: vec![CreateNotificationItem {
                    user_id: UserId::new(),
                    notification_payload: payload(),
                    external: true,
                }],
            })
            .await;

        assert!(matches!(
            result,
            Err(CreateNotificationsError::InsertFailed(_))
        ));
    }
}
