use super::mapping::{PAYLOAD_VERSION, encode_payload};
use crate::delivery_mapping::channel_to_persisted;
use application::error::box_error;
use notification_core::{
    external_email::ExternalEmailOrigin,
    external_email_message_id::ExternalEmailMessageId,
    notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    notification_delivery_id::NotificationDeliveryId,
};
use notification_service::ports::external_email_outbox::{
    ExternalEmailEnqueueOutcome, ExternalEmailErasure, ExternalEmailOutbox,
    ExternalEmailOutboxError, ExternalEmailOutboxFactory, ExternalEmailRequest,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxExternalEmailOutboxFactory;

impl SqlxExternalEmailOutboxFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ExternalEmailOutboxFactory<SqlxTransaction> for SqlxExternalEmailOutboxFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ExternalEmailOutbox + 'tx {
        SqlxExternalEmailOutbox {
            connection: tx.connection(),
        }
    }
}

struct SqlxExternalEmailOutbox<'tx> {
    connection: &'tx mut PgConnection,
}

#[async_trait::async_trait]
impl ExternalEmailOutbox for SqlxExternalEmailOutbox<'_> {
    async fn enqueue(
        &mut self,
        request: ExternalEmailRequest,
    ) -> Result<ExternalEmailEnqueueOutcome, ExternalEmailOutboxError> {
        let template = request.content.template();
        let payload = encode_payload(&request.content).map_err(|source| {
            ExternalEmailOutboxError::OperationFailed {
                source: box_error(source),
            }
        })?;
        // A concurrent enqueue of the same origin and template waits for that row's
        // transaction and then finds it, so one origin never enqueues an email twice. The
        // retry covers an erasure that removed the conflicting row in between.
        for _ in 0..2 {
            let external_email_message_id = ExternalEmailMessageId::new();
            let inserted = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO external_email_messages (external_email_message_id, recipient_email, language, template, payload_version, payload, origin_type, origin_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT ON CONSTRAINT external_email_messages_origin_template_unique DO NOTHING RETURNING external_email_message_id",
            )
            .bind(external_email_message_id.into_uuid())
            .bind(request.recipient.as_str())
            .bind(request.language.as_str())
            .bind(template.as_str())
            .bind(PAYLOAD_VERSION)
            .bind(&payload)
            .bind(request.origin.origin_type())
            .bind(request.origin.origin_id())
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(operation_failed)?;
            if inserted.is_some() {
                self.insert_delivery(external_email_message_id).await?;
                return Ok(ExternalEmailEnqueueOutcome::Enqueued(
                    external_email_message_id,
                ));
            }
            if let Some(existing) = self
                .existing_message_id(request.origin, template.as_str())
                .await?
            {
                return Ok(ExternalEmailEnqueueOutcome::AlreadyEnqueued(existing));
            }
        }
        Err(ExternalEmailOutboxError::OperationFailed {
            source: box_error(std::io::Error::other(
                "external email message conflicted but could not be found",
            )),
        })
    }

    async fn erase_origin(
        &mut self,
        origin: ExternalEmailOrigin,
    ) -> Result<ExternalEmailErasure, ExternalEmailOutboxError> {
        // DELIVERED and FAILED are final: no claim can move a delivery out of them.
        let (erased, in_flight) = sqlx::query_as::<_, (i64, i64)>(
            r#"
            WITH erased AS (
                DELETE FROM external_email_messages m
                WHERE m.origin_type = $1 AND m.origin_id = $2
                  AND NOT EXISTS (
                      SELECT 1 FROM notification_deliveries d
                      WHERE d.external_email_message_id = m.external_email_message_id
                        AND d.status NOT IN ('DELIVERED', 'FAILED'))
                RETURNING m.external_email_message_id
            )
            SELECT
                (SELECT count(*) FROM erased),
                (SELECT count(*) FROM external_email_messages m
                 WHERE m.origin_type = $1 AND m.origin_id = $2
                   AND m.external_email_message_id NOT IN (SELECT external_email_message_id FROM erased))
            "#,
        )
        .bind(origin.origin_type())
        .bind(origin.origin_id())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        Ok(ExternalEmailErasure {
            erased: count(erased)?,
            in_flight: count(in_flight)?,
        })
    }
}

impl SqlxExternalEmailOutbox<'_> {
    async fn insert_delivery(
        &mut self,
        external_email_message_id: ExternalEmailMessageId,
    ) -> Result<(), ExternalEmailOutboxError> {
        sqlx::query(
            "INSERT INTO notification_deliveries (notification_delivery_id, external_email_message_id, channel, target_key) VALUES ($1, $2, $3, $4)",
        )
        .bind(NotificationDeliveryId::new().into_uuid())
        .bind(external_email_message_id.into_uuid())
        .bind(channel_to_persisted(NotificationDeliveryChannel::Email))
        .bind(NotificationDeliveryTargetKey::recipient().as_str())
        .execute(&mut *self.connection)
        .await
        .map(|_| ())
        .map_err(operation_failed)
    }

    async fn existing_message_id(
        &mut self,
        origin: ExternalEmailOrigin,
        template: &str,
    ) -> Result<Option<ExternalEmailMessageId>, ExternalEmailOutboxError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            "SELECT external_email_message_id FROM external_email_messages WHERE origin_type = $1 AND origin_id = $2 AND template = $3",
        )
        .bind(origin.origin_type())
        .bind(origin.origin_id())
        .bind(template)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        id.map(ExternalEmailMessageId::try_from)
            .transpose()
            .map_err(|source| ExternalEmailOutboxError::InvalidPersistedState {
                source: box_error(source),
            })
    }
}

fn count(value: i64) -> Result<u64, ExternalEmailOutboxError> {
    u64::try_from(value).map_err(|source| ExternalEmailOutboxError::InvalidPersistedState {
        source: box_error(source),
    })
}

fn operation_failed(source: sqlx::Error) -> ExternalEmailOutboxError {
    ExternalEmailOutboxError::OperationFailed {
        source: box_error(source),
    }
}
