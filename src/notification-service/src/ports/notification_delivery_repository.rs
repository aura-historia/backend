use common::{
    currency::domain::Currency, error::boxed::BoxError, language::domain::Language,
    notification_id::NotificationId, user_id::UserId,
};
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
    pub recipient_first_name: Option<String>,
    pub language: Language,
    pub currency: Currency,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimNotificationDeliveryOutcome {
    Claimed {
        delivery: ClaimedNotificationDelivery,
        source: Box<Option<NotificationDeliverySource>>,
    },
    Missing,
    Delivered,
    PermanentlyFailed,
    AlreadyClaimed,
    NotificationMismatch,
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
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait NotificationDeliveryRepository: Send + Sync {
    async fn claim_and_load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        notification_id: NotificationId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError>;

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
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError>;
}
