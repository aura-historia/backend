use application::error::BoxError;
use domain_primitives::event_id::EventId;
use notification_core::{
    notification::NotificationPayload, notification_id::NotificationId,
    notification_type::NotificationType,
};
use product_core::product_id::ProductId;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct AllNotificationsReadItem {
    pub user_id: UserId,
    pub origin_event_id: EventId,
    pub notification_id: NotificationId,
    pub notification_type: Option<NotificationType>,
    pub notification_payload: NotificationPayload,
    pub seen: bool,
    pub external: bool,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl AllNotificationsReadItem {
    pub fn product_id(&self) -> Option<ProductId> {
        match &self.notification_payload {
            NotificationPayload::Watchlist { product_id, .. }
            | NotificationPayload::SearchFilter { product_id, .. } => Some(*product_id),
            NotificationPayload::PartnerApplication { .. } => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AllNotificationsReadError {
    #[error("notification read failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted notification read model is invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait AllNotificationsReader: Send + Sync {
    async fn list_all_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Vec<AllNotificationsReadItem>, AllNotificationsReadError>;
}
