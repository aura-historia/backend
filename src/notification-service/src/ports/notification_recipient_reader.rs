use common::{error::boxed::BoxError, user_id::UserId};
use localization::Language;
use money::Currency;

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationRecipient {
    pub user_id: UserId,
    pub email: String,
    pub first_name: Option<String>,
    pub languages: Vec<Language>,
    pub currency: Currency,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationRecipientReadError {
    #[error("notification recipient lookup failed")]
    LookupFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationRecipientReader: Send + Sync {
    async fn find_recipient(
        &self,
        user_id: &UserId,
    ) -> Result<Option<NotificationRecipient>, NotificationRecipientReadError>;
}
