use crate::ports::notification_list_reader::{
    NotificationListCursor, NotificationListReadError, NotificationListReader,
};
use common::{
    currency::domain::Currency, language::domain::Language, notification_id::NotificationId,
    user_id::UserId,
};
use notification_core::notification::LocalizedNotificationContent;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct ListNotificationsRequest {
    pub user_id: UserId,
    pub languages: Vec<Language>,
    pub currency: Currency,
    pub cursor: Option<NotificationListCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListedNotification {
    pub notification_id: NotificationId,
    pub content: LocalizedNotificationContent,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListNotificationsResult {
    pub items: Vec<ListedNotification>,
    pub next_cursor: Option<NotificationListCursor>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListNotificationsError {
    #[error("notification list read failed")]
    ReadFailed(#[from] NotificationListReadError),
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
    R: NotificationListReader,
{
    async fn execute(
        &self,
        request: ListNotificationsRequest,
    ) -> Result<ListNotificationsResult, ListNotificationsError> {
        let page = self
            .reader
            .list_for_user(request.user_id, request.cursor, request.limit)
            .await?;
        let items = page
            .items
            .into_iter()
            .map(|item| ListedNotification {
                notification_id: item.notification_id,
                content: item
                    .content
                    .localized(&request.currency, &request.languages),
                seen: item.seen,
                created: item.created,
                updated: item.updated,
            })
            .collect();
        Ok(ListNotificationsResult {
            items,
            next_cursor: page.next_cursor,
        })
    }
}
