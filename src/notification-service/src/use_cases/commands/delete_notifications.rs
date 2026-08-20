use crate::ports::{
    all_notifications_reader::{AllNotificationsReadError, AllNotificationsReader},
    notification_deleter::{NotificationDeleteError, NotificationDeleter},
};
use domain_primitives::event_id::EventId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeleteNotificationsCommand {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteNotificationsResult {
    pub deleted: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteNotificationsError {
    #[error("notification list failed")]
    ReadFailed(#[source] AllNotificationsReadError),
    #[error("notification delete failed")]
    DeleteFailed(#[source] NotificationDeleteError),
}

#[async_trait::async_trait]
pub trait DeleteNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        command: DeleteNotificationsCommand,
    ) -> Result<DeleteNotificationsResult, DeleteNotificationsError>;
}

pub struct DeleteNotificationsHandler<R, D> {
    reader: R,
    deleter: D,
}

impl<R, D> DeleteNotificationsHandler<R, D> {
    pub fn new(reader: R, deleter: D) -> Self {
        Self { reader, deleter }
    }
}

#[async_trait::async_trait]
impl<R, D> DeleteNotificationsUseCase for DeleteNotificationsHandler<R, D>
where
    R: AllNotificationsReader,
    D: NotificationDeleter,
{
    async fn execute(
        &self,
        command: DeleteNotificationsCommand,
    ) -> Result<DeleteNotificationsResult, DeleteNotificationsError> {
        let items = self
            .reader
            .list_all_by_user(&command.user_id)
            .await
            .map_err(DeleteNotificationsError::ReadFailed)?;
        let origin_event_ids = items
            .iter()
            .map(|item| item.origin_event_id)
            .collect::<Vec<EventId>>();
        self.deleter
            .delete_many_by_origin_event_id(&command.user_id, &origin_event_ids)
            .await
            .map_err(DeleteNotificationsError::DeleteFailed)?;
        Ok(DeleteNotificationsResult {
            deleted: origin_event_ids.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::all_notifications_reader::AllNotificationsReadItem;
    use application::error::{BoxError, box_error};
    use notification_core::{
        notification::{NotificationPartnerApplicationPayload, NotificationPayload},
        notification_id::NotificationId,
    };
    use shop_core::shop_name::ShopName;
    use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

    #[derive(Clone, Default)]
    struct FakeGateway {
        items: Arc<Mutex<Vec<AllNotificationsReadItem>>>,
        deleted: Arc<Mutex<Vec<EventId>>>,
    }

    fn item(user_id: UserId, origin_event_id: EventId) -> AllNotificationsReadItem {
        AllNotificationsReadItem {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload: NotificationPayload::PartnerApplication {
                shop_name: ShopName::from("test shop"),
                image: None,
                partner_application_payload: NotificationPartnerApplicationPayload::Approved {
                    partner_application_id: PartnerShopApplicationId::new(),
                },
            },
            seen: false,
            external: true,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    #[async_trait::async_trait]
    impl AllNotificationsReader for FakeGateway {
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
    impl NotificationDeleter for FakeGateway {
        async fn delete_by_origin_event_id(
            &self,
            _user_id: &UserId,
            _origin_event_id: &EventId,
        ) -> Result<(), NotificationDeleteError> {
            Ok(())
        }

        async fn delete_many_by_origin_event_id(
            &self,
            _user_id: &UserId,
            origin_event_ids: &[EventId],
        ) -> Result<(), NotificationDeleteError> {
            self.deleted
                .lock()
                .unwrap()
                .extend_from_slice(origin_event_ids);
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_delete_all_notifications_for_user() {
        let gateway = FakeGateway::default();
        let user_id = UserId::new();
        let first = EventId::new();
        let second = EventId::new();
        *gateway.items.lock().unwrap() = vec![item(user_id, first), item(user_id, second)];

        let result = DeleteNotificationsHandler::new(gateway.clone(), gateway.clone())
            .execute(DeleteNotificationsCommand { user_id })
            .await
            .expect("delete all should succeed");

        assert_eq!(2, result.deleted);
        assert_eq!(vec![first, second], *gateway.deleted.lock().unwrap());
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
    async fn should_fail_delete_all_notifications_when_read_fails() {
        let result = DeleteNotificationsHandler::new(FailingReader, FakeGateway::default())
            .execute(DeleteNotificationsCommand {
                user_id: UserId::new(),
            })
            .await;

        assert!(matches!(
            result,
            Err(DeleteNotificationsError::ReadFailed(_))
        ));
    }
}
