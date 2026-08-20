use common::{error::boxed::BoxError, event_id::EventId, user_id::UserId};
use notification_core::{
    notification::NotificationPayload, notification_id::NotificationId,
    notification_type::NotificationType,
};
use product_core::product_id::ProductId;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductNotificationReadItem {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub notification_type: Option<NotificationType>,
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub external: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductNotificationsReadError {
    #[error("product notification read failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted product notification read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait ProductNotificationsReader: Send + Sync {
    async fn list_by_product(
        &self,
        user_id: &UserId,
        product_id: &ProductId,
        limit: Option<i32>,
        newest_first: bool,
    ) -> Result<Vec<ProductNotificationReadItem>, ProductNotificationsReadError>;
}
