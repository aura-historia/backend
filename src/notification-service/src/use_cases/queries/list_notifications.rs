use crate::ports::notification_list_reader::{
    NotificationListCursor, NotificationListReadError, NotificationListReader,
};
use crate::presentation::NotificationPresentationPreferences;
use application::operation_context::{OperationAuthorizationError, OperationContext, Principal};
use localization::Language;
use notification_core::notification_id::NotificationId;
use notification_core::{
    notification::LocalizedNotificationContent, notification_kind::NotificationKind,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListNotificationsRequest {
    pub languages: Vec<Language>,
    pub cursor: Option<NotificationListCursor>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListedNotification {
    pub notification_id: NotificationId,
    pub kind: NotificationKind,
    pub content: LocalizedNotificationContent,
    pub seen: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListNotificationsResult {
    pub items: Vec<ListedNotification>,
    pub next_cursor: Option<NotificationListCursor>,
    pub presentation_preferences: NotificationPresentationPreferences,
}

#[derive(Debug, thiserror::Error)]
pub enum ListNotificationsError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("notification list read failed")]
    ReadFailed(#[from] NotificationListReadError),
}

#[async_trait::async_trait]
pub trait ListNotificationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
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
        context: &OperationContext,
        request: ListNotificationsRequest,
    ) -> Result<ListNotificationsResult, ListNotificationsError> {
        let user_id = notification_owner(context)?;
        let page = self
            .reader
            .list_for_user(user_id, request.cursor, request.limit)
            .await?;
        let items = page
            .items
            .into_iter()
            .map(|item| {
                let kind = item.content.kind();
                let content = item.content.localized(&request.languages);

                ListedNotification {
                    notification_id: item.notification_id,
                    kind,
                    content,
                    seen: item.seen,
                    created: item.created,
                    updated: item.updated,
                }
            })
            .collect();
        Ok(ListNotificationsResult {
            items,
            next_cursor: page.next_cursor,
            presentation_preferences: NotificationPresentationPreferences {
                language: request.languages.first().copied().unwrap_or(Language::En),
                prohibited_content_consent: page.prohibited_content_consent,
            },
        })
    }
}

fn notification_owner(context: &OperationContext) -> Result<UserId, ListNotificationsError> {
    context
        .require()
        .any_user()
        .authorize::<ListNotificationsError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(ListNotificationsError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for ListNotificationsError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
