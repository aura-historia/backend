use crate::mapping::{NotificationRow, mapping_error};
use common::{error::boxed::box_error, notification_id::NotificationId};
use notification_core::notification_delivery_id::NotificationDeliveryId;
use notification_service::ports::notification_delivery_repository::{
    ClaimedNotificationDelivery, NotificationDeliveryError, NotificationDeliveryRepository,
    NotificationDeliverySource,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SqlxNotificationDeliveryRepository {
    pool: PgPool,
}
impl SqlxNotificationDeliveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct DeliveryClaimRow {
    notification_delivery_id: Uuid,
    notification_id: Uuid,
    lease_token: Uuid,
    lease_expires_at: OffsetDateTime,
    attempt_count: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct DeliverySourceRow {
    recipient_email: String,
    notification_id: Uuid,
    user_id: Uuid,
    kind: String,
    origin_event_id: Option<Uuid>,
    product_id: Option<Uuid>,
    user_search_filter_id: Option<Uuid>,
    partner_shop_application_id: Option<Uuid>,
    payload_version: i16,
    payload: serde_json::Value,
    seen: bool,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[async_trait::async_trait]
impl NotificationDeliveryRepository for SqlxNotificationDeliveryRepository {
    async fn claim(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<Option<ClaimedNotificationDelivery>, NotificationDeliveryError> {
        let row = sqlx::query_as::<_, DeliveryClaimRow>(
            "UPDATE notification_deliveries SET status = 'PROCESSING', lease_token = $2, lease_expires_at = $3, attempt_count = attempt_count + 1, updated = now() WHERE notification_delivery_id = $1 AND (status = 'PENDING' OR (status = 'PROCESSING' AND lease_expires_at <= $4)) RETURNING notification_delivery_id, notification_id, lease_token, lease_expires_at, attempt_count",
        ).bind(uuid::Uuid::from(notification_delivery_id)).bind(lease_token).bind(lease_expires_at).bind(now).fetch_optional(&self.pool).await
            .map_err(|source| NotificationDeliveryError::OperationFailed { source: box_error(source) })?;
        row.map(|row| {
            let attempt_count = u32::try_from(row.attempt_count).map_err(|source| {
                NotificationDeliveryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
            Ok(ClaimedNotificationDelivery {
                notification_delivery_id: NotificationDeliveryId::from(
                    row.notification_delivery_id,
                ),
                notification_id: NotificationId::from(row.notification_id),
                lease_token: row.lease_token,
                lease_expires_at: row.lease_expires_at,
                attempt_count,
            })
        })
        .transpose()
    }

    async fn load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
    ) -> Result<Option<NotificationDeliverySource>, NotificationDeliveryError> {
        let row = sqlx::query_as::<_, DeliverySourceRow>(
            "SELECT u.email AS recipient_email, n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_id, n.user_search_filter_id, n.partner_shop_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated FROM notification_deliveries d JOIN notifications n ON n.notification_id = d.notification_id JOIN users u ON u.user_id = n.user_id WHERE d.notification_delivery_id = $1",
        ).bind(uuid::Uuid::from(notification_delivery_id)).fetch_optional(&self.pool).await
            .map_err(|source| NotificationDeliveryError::OperationFailed { source: box_error(source) })?;
        row.map(|row| {
            let notification =
                notification_core::notification::Notification::try_from(NotificationRow {
                    notification_id: row.notification_id,
                    user_id: row.user_id,
                    kind: row.kind,
                    origin_event_id: row.origin_event_id,
                    product_id: row.product_id,
                    user_search_filter_id: row.user_search_filter_id,
                    partner_shop_application_id: row.partner_shop_application_id,
                    payload_version: row.payload_version,
                    payload: row.payload,
                    seen: row.seen,
                    created: row.created,
                    updated: row.updated,
                })
                .map_err(|error| {
                    NotificationDeliveryError::InvalidPersistedState {
                        source: mapping_error(error),
                    }
                })?;
            Ok(NotificationDeliverySource {
                notification_delivery_id,
                notification_id: notification.notification_id(),
                user_id: notification.user_id(),
                content: notification.content().clone(),
                recipient_email: row.recipient_email,
            })
        })
        .transpose()
    }

    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        provider_message_id: &str,
        delivered_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(&self.pool, "UPDATE notification_deliveries SET status = 'DELIVERED', lease_token = NULL, lease_expires_at = NULL, provider_message_id = $3, delivered_at = $4, updated = now() WHERE notification_delivery_id = $1 AND status = 'PROCESSING' AND lease_token = $2", notification_delivery_id, lease_token, provider_message_id, Some(delivered_at)).await
    }
    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(&self.pool, "UPDATE notification_deliveries SET status = 'PENDING', lease_token = NULL, lease_expires_at = NULL, last_error_code = $3, updated = now() WHERE notification_delivery_id = $1 AND status = 'PROCESSING' AND lease_token = $2", notification_delivery_id, lease_token, error_code, None).await
    }
    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(&self.pool, "UPDATE notification_deliveries SET status = 'FAILED', lease_token = NULL, lease_expires_at = NULL, last_error_code = $3, updated = now() WHERE notification_delivery_id = $1 AND status = 'PROCESSING' AND lease_token = $2", notification_delivery_id, lease_token, error_code, None).await
    }
}

async fn complete(
    pool: &PgPool,
    sql: &str,
    id: NotificationDeliveryId,
    lease_token: Uuid,
    value: &str,
    delivered_at: Option<OffsetDateTime>,
) -> Result<bool, NotificationDeliveryError> {
    let mut query = sqlx::query(sql)
        .bind(uuid::Uuid::from(id))
        .bind(lease_token)
        .bind(value);
    if let Some(delivered_at) = delivered_at {
        query = query.bind(delivered_at);
    }
    let result =
        query
            .execute(pool)
            .await
            .map_err(|source| NotificationDeliveryError::OperationFailed {
                source: box_error(source),
            })?;
    Ok(result.rows_affected() == 1)
}
