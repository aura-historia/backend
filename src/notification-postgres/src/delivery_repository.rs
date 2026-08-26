use crate::{
    delivery_mapping::channel_from_persisted,
    mapping::{NotificationRow, content_policy_from_assessment, mapping_error},
};
use application::error::box_error;
use localization::Language;
use notification_core::{
    notification_delivery::NotificationDeliveryTargetKey,
    notification_delivery_id::NotificationDeliveryId, notification_id::NotificationId,
};
use notification_service::ports::notification_delivery_repository::{
    ClaimNotificationDeliveryOutcome, ClaimedNotificationDelivery, NotificationDeliveryError,
    NotificationDeliveryRepository, NotificationDeliverySource,
};
use notification_service::presentation::NotificationPresentationPreferences;
use sqlx::{PgPool, Postgres, QueryBuilder};
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
struct DeliveryStatusRow {
    status: String,
}

#[derive(Debug, sqlx::FromRow)]
struct DeliverySourceRow {
    notification_delivery_id: Uuid,
    channel: String,
    target_key: String,
    language: Option<String>,
    show_unassessed_or_sensitive_content: bool,
    notification_id: Uuid,
    user_id: Uuid,
    kind: String,
    origin_event_id: Option<Uuid>,
    product_listing_id: Option<Uuid>,
    user_search_filter_id: Option<Uuid>,
    partner_shop_application_id: Option<Uuid>,
    payload_version: i16,
    payload: serde_json::Value,
    seen: bool,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    content_policy_decision: Option<String>,
    content_policy_category: Option<String>,
}

#[async_trait::async_trait]
impl NotificationDeliveryRepository for SqlxNotificationDeliveryRepository {
    async fn claim_and_load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
        let mut transaction = self.pool.begin().await.map_err(operation_error)?;
        let row = sqlx::query_as::<_, DeliveryClaimRow>(
            "UPDATE notification_deliveries SET status = 'PROCESSING', lease_token = $2, lease_expires_at = $3, attempt_count = attempt_count + 1, updated = now() WHERE notification_delivery_id = $1 AND (status = 'PENDING' OR (status = 'PROCESSING' AND lease_expires_at <= $4)) RETURNING notification_delivery_id, notification_id, lease_token, lease_expires_at, attempt_count",
        )
        .bind(Uuid::from(notification_delivery_id))
        .bind(lease_token)
        .bind(lease_expires_at)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(operation_error)?;

        let Some(row) = row else {
            let status = sqlx::query_as::<_, DeliveryStatusRow>(
                "SELECT status FROM notification_deliveries WHERE notification_delivery_id = $1",
            )
            .bind(Uuid::from(notification_delivery_id))
            .fetch_optional(&mut *transaction)
            .await
            .map_err(operation_error)?;
            transaction.commit().await.map_err(operation_error)?;
            return match status {
                None => Ok(ClaimNotificationDeliveryOutcome::Missing),
                Some(DeliveryStatusRow { status }) if status == "DELIVERED" => {
                    Ok(ClaimNotificationDeliveryOutcome::Delivered)
                }
                Some(DeliveryStatusRow { status }) if status == "FAILED" => {
                    Ok(ClaimNotificationDeliveryOutcome::PermanentlyFailed)
                }
                Some(DeliveryStatusRow { status })
                    if matches!(status.as_str(), "PENDING" | "PROCESSING") =>
                {
                    Ok(ClaimNotificationDeliveryOutcome::AlreadyClaimed)
                }
                Some(_) => Err(invalid_delivery_source(
                    "unknown notification delivery status",
                )),
            };
        };

        let claimed = claimed_from_row(row)?;
        let source = sqlx::query_as::<_, DeliverySourceRow>(
            "SELECT d.notification_delivery_id, d.channel, d.target_key, u.language, u.show_unassessed_or_sensitive_content, n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_listing_id, n.user_search_filter_id, n.partner_shop_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated, a.decision AS content_policy_decision, a.category AS content_policy_category FROM notification_deliveries d JOIN notifications n ON n.notification_id = d.notification_id JOIN users u ON u.user_id = n.user_id LEFT JOIN product_listings p ON p.product_listing_id = n.product_listing_id LEFT JOIN product_listing_content_assessments a ON a.product_listing_id = p.product_listing_id AND a.source_event_id = p.content_source_event_id WHERE d.notification_delivery_id = $1",
        )
        .bind(Uuid::from(notification_delivery_id))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(operation_error)?;
        let source = source.map(source_from_row).transpose()?;
        transaction.commit().await.map_err(operation_error)?;

        Ok(ClaimNotificationDeliveryOutcome::Claimed {
            delivery: claimed,
            source: Box::new(source),
        })
    }

    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        provider_message_id: &str,
        delivered_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::Delivered {
                provider_message_id,
                delivered_at,
            },
            delivered_at,
        )
        .await
    }

    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::RetryableFailure { error_code },
            completed_at,
        )
        .await
    }

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::PermanentFailure { error_code },
            completed_at,
        )
        .await
    }
}

fn operation_error(source: sqlx::Error) -> NotificationDeliveryError {
    NotificationDeliveryError::OperationFailed {
        source: box_error(source),
    }
}

fn claimed_from_row(
    row: DeliveryClaimRow,
) -> Result<ClaimedNotificationDelivery, NotificationDeliveryError> {
    let attempt_count = u32::try_from(row.attempt_count).map_err(|source| {
        NotificationDeliveryError::InvalidPersistedState {
            source: box_error(source),
        }
    })?;
    Ok(ClaimedNotificationDelivery {
        notification_delivery_id: NotificationDeliveryId::from(row.notification_delivery_id),
        notification_id: NotificationId::from(row.notification_id),
        lease_token: row.lease_token,
        lease_expires_at: row.lease_expires_at,
        attempt_count,
    })
}

fn source_from_row(
    row: DeliverySourceRow,
) -> Result<NotificationDeliverySource, NotificationDeliveryError> {
    let content_policy = content_policy_from_assessment(
        row.content_policy_decision.clone(),
        row.content_policy_category.clone(),
    )
    .map_err(|error| NotificationDeliveryError::InvalidPersistedState {
        source: mapping_error(error),
    })?;
    let mut notification =
        notification_core::notification::Notification::try_from(NotificationRow {
            notification_id: row.notification_id,
            user_id: row.user_id,
            kind: row.kind,
            origin_event_id: row.origin_event_id,
            product_listing_id: row.product_listing_id,
            user_search_filter_id: row.user_search_filter_id,
            partner_shop_application_id: row.partner_shop_application_id,
            payload_version: row.payload_version,
            payload: row.payload,
            seen: row.seen,
            created: row.created,
            updated: row.updated,
        })
        .map_err(|error| NotificationDeliveryError::InvalidPersistedState {
            source: mapping_error(error),
        })?;
    notification.set_product_listing_content_policy(content_policy);

    Ok(NotificationDeliverySource {
        notification_delivery_id: NotificationDeliveryId::from(row.notification_delivery_id),
        notification_id: notification.notification_id(),
        user_id: notification.user_id(),
        channel: channel_from_persisted(&row.channel).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?,
        target_key: NotificationDeliveryTargetKey::try_from(row.target_key).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?,
        content: notification.content().clone(),
        presentation_preferences: NotificationPresentationPreferences {
            language: row
                .language
                .as_deref()
                .map(parse_language)
                .transpose()?
                .unwrap_or(Language::En),
            show_unassessed_or_sensitive_content: row.show_unassessed_or_sensitive_content,
        },
    })
}

fn parse_language(value: &str) -> Result<Language, NotificationDeliveryError> {
    Language::from_code(value).ok_or_else(|| invalid_delivery_source("unknown user language"))
}

fn invalid_delivery_source(message: &'static str) -> NotificationDeliveryError {
    NotificationDeliveryError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}

enum DeliveryCompletion<'a> {
    Delivered {
        provider_message_id: &'a str,
        delivered_at: OffsetDateTime,
    },
    RetryableFailure {
        error_code: &'a str,
    },
    PermanentFailure {
        error_code: &'a str,
    },
}

async fn complete(
    pool: &PgPool,
    id: NotificationDeliveryId,
    lease_token: Uuid,
    completion: DeliveryCompletion<'_>,
    completed_at: OffsetDateTime,
) -> Result<bool, NotificationDeliveryError> {
    let mut query = QueryBuilder::<Postgres>::new("UPDATE notification_deliveries SET status = ");
    match completion {
        DeliveryCompletion::Delivered {
            provider_message_id,
            delivered_at,
        } => {
            query
                .push("'DELIVERED', lease_token = NULL, lease_expires_at = NULL, provider_message_id = ")
                .push_bind(provider_message_id)
                .push(", last_error_code = NULL, delivered_at = ")
                .push_bind(delivered_at);
        }
        DeliveryCompletion::RetryableFailure { error_code } => {
            query
                .push("'PENDING', lease_token = NULL, lease_expires_at = NULL, provider_message_id = NULL, last_error_code = ")
                .push_bind(error_code)
                .push(", delivered_at = NULL");
        }
        DeliveryCompletion::PermanentFailure { error_code } => {
            query
                .push("'FAILED', lease_token = NULL, lease_expires_at = NULL, provider_message_id = NULL, last_error_code = ")
                .push_bind(error_code)
                .push(", delivered_at = NULL");
        }
    }
    let result = query
        .push(", updated = now() WHERE notification_delivery_id = ")
        .push_bind(Uuid::from(id))
        .push(" AND status = 'PROCESSING' AND lease_token = ")
        .push_bind(lease_token)
        .push(" AND lease_expires_at > ")
        .push_bind(completed_at)
        .build()
        .execute(pool)
        .await
        .map_err(operation_error)?;
    Ok(result.rows_affected() == 1)
}
