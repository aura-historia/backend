use crate::ports::list_notifications_reader::{
    ListNotificationsReadError, ListNotificationsReader,
};
use common::{
    event_id::EventId,
    pagination::cursor::{Cursor, CursoredResult},
    user_id::UserId,
};
use localization::Language;
use money::Currency;
use notification_core::{
    notification::LocalizedNotificationPayload, notification_id::NotificationId,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListNotificationsRequest {
    pub user_id: UserId,
    pub languages: Vec<Language>,
    pub currency: Currency,
    pub cursor: Option<Cursor<EventId>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListedNotification {
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub payload: LocalizedNotificationPayload,
    pub seen: bool,
    pub external: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type ListNotificationsResult = CursoredResult<ListedNotification, EventId>;

#[derive(Debug, thiserror::Error)]
pub enum ListNotificationsError {
    #[error("notification list failed")]
    ReadFailed(#[source] ListNotificationsReadError),
}

#[async_trait::async_trait]
pub trait ListNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        request: ListNotificationsRequest,
    ) -> Result<ListNotificationsResult, ListNotificationsError>;
}

pub struct ListNotificationsHandler<R> {
    reader: R,
}

impl<R> ListNotificationsHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}

#[async_trait::async_trait]
impl<R> ListNotificationsUseCase for ListNotificationsHandler<R>
where
    R: ListNotificationsReader,
{
    async fn execute(
        &self,
        request: ListNotificationsRequest,
    ) -> Result<ListNotificationsResult, ListNotificationsError> {
        let cursor = request.cursor.unwrap_or_default();
        let newest_first = true;
        let rows = self
            .reader
            .list_by_user(&request.user_id, &cursor, newest_first)
            .await
            .map_err(ListNotificationsError::ReadFailed)?;
        let last = rows.last().map(|row| row.origin_event_id);
        let items = rows
            .into_iter()
            .map(|row| ListedNotification {
                origin_event_id: row.origin_event_id,
                notification_id: row.notification_id,
                payload: row
                    .notification_payload
                    .localized(&request.currency, &request.languages),
                seen: row.seen,
                external: row.external,
                created: row.created,
                updated: row.updated,
            })
            .collect::<Vec<_>>();
        let total = self
            .reader
            .count_by_user(&request.user_id, &cursor, newest_first)
            .await
            .map_err(ListNotificationsError::ReadFailed)?;
        Ok(CursoredResult {
            cursor: Cursor {
                size: items.len() as u64,
                search_after: last,
            },
            total: Some(total),
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::list_notifications_reader::NotificationListReadItem;
    use common::{partner_shop_application_id::PartnerShopApplicationId, shop_name::ShopName};
    use notification_core::{
        notification::NotificationPartnerApplicationPayload, notification_id::NotificationId,
    };

    #[derive(Clone)]
    struct FakeReader {
        items: Vec<NotificationListReadItem>,
        count: u64,
    }

    fn item(user_id: UserId, origin_event_id: EventId) -> NotificationListReadItem {
        NotificationListReadItem {
            user_id,
            origin_event_id,
            notification_id: NotificationId::new(),
            notification_type: None,
            notification_payload:
                notification_core::notification::NotificationPayload::PartnerApplication {
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
    impl ListNotificationsReader for FakeReader {
        async fn list_by_user(
            &self,
            user_id: &UserId,
            _cursor: &Cursor<EventId>,
            _newest_first: bool,
        ) -> Result<Vec<NotificationListReadItem>, ListNotificationsReadError> {
            Ok(self
                .items
                .iter()
                .filter(|item| item.user_id == *user_id)
                .cloned()
                .collect())
        }

        async fn count_by_user(
            &self,
            _user_id: &UserId,
            _cursor: &Cursor<EventId>,
            _newest_first: bool,
        ) -> Result<u64, ListNotificationsReadError> {
            Ok(self.count)
        }
    }

    #[tokio::test]
    async fn should_list_notifications_with_use_case_view() {
        let user_id = UserId::new();
        let request = ListNotificationsRequest {
            user_id,
            languages: vec![Language::En],
            currency: Currency::Eur,
            cursor: None,
        };

        let result = ListNotificationsHandler::new(FakeReader {
            items: vec![item(user_id, EventId::new())],
            count: 1,
        })
        .execute(request)
        .await
        .expect("list should succeed");

        assert_eq!(1, result.items.len());
        assert_eq!(Some(1), result.total);
    }
}
