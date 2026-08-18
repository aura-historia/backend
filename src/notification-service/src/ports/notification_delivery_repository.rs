use common::{error::boxed::BoxError, notification_id::NotificationId, user_id::UserId};
use notification_core::notification::NotificationContent;
use notification_core::notification_delivery_id::NotificationDeliveryId;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationDeliveryStatus {
    Pending,
    Processing,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedNotificationDelivery {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
    pub lease_token: Uuid,
    pub lease_expires_at: OffsetDateTime,
    pub attempt_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationDeliverySource {
    pub notification_delivery_id: NotificationDeliveryId,
    pub notification_id: NotificationId,
    pub user_id: UserId,
    pub content: NotificationContent,
    pub recipient_email: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeliveryError {
    #[error("notification delivery operation failed")]
    OperationFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted notification delivery state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait NotificationDeliveryRepository: Send + Sync {
    async fn claim(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<Option<ClaimedNotificationDelivery>, NotificationDeliveryError>;

    async fn load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
    ) -> Result<Option<NotificationDeliverySource>, NotificationDeliveryError>;

    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        provider_message_id: &str,
        delivered_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;

    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
    ) -> Result<bool, NotificationDeliveryError>;

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
    ) -> Result<bool, NotificationDeliveryError>;
}
