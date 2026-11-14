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
    ExternalEmailEnqueueOutcome, ExternalEmailOutbox, ExternalEmailOutboxError,
    ExternalEmailOutboxFactory, ExternalEmailRequest,
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
        let external_email_message_id = ExternalEmailMessageId::new();
        // A concurrent enqueue of the same origin and template waits for this row's
        // transaction and then finds it, so one origin never enqueues an email twice.
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO external_email_messages (external_email_message_id, recipient_email, language, template, payload_version, payload, origin_type, origin_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT ON CONSTRAINT external_email_messages_origin_template_unique DO NOTHING RETURNING external_email_message_id",
        )
        .bind(external_email_message_id.into_uuid())
        .bind(request.recipient.as_str())
        .bind(request.language.as_str())
        .bind(template.as_str())
        .bind(PAYLOAD_VERSION)
        .bind(payload)
        .bind(request.origin.origin_type())
        .bind(request.origin.origin_id())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(operation_failed)?;

        if inserted.is_none() {
            return self
                .existing_message_id(request.origin, template.as_str())
                .await
                .map(ExternalEmailEnqueueOutcome::AlreadyEnqueued);
        }
        sqlx::query(
            "INSERT INTO notification_deliveries (notification_delivery_id, external_email_message_id, channel, target_key) VALUES ($1, $2, $3, $4)",
        )
        .bind(NotificationDeliveryId::new().into_uuid())
        .bind(external_email_message_id.into_uuid())
        .bind(channel_to_persisted(NotificationDeliveryChannel::Email))
        .bind(NotificationDeliveryTargetKey::recipient().as_str())
        .execute(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        Ok(ExternalEmailEnqueueOutcome::Enqueued(
            external_email_message_id,
        ))
    }

    async fn erase_origin(
        &mut self,
        origin: ExternalEmailOrigin,
    ) -> Result<u64, ExternalEmailOutboxError> {
        sqlx::query("DELETE FROM external_email_messages WHERE origin_type = $1 AND origin_id = $2")
            .bind(origin.origin_type())
            .bind(origin.origin_id())
            .execute(&mut *self.connection)
            .await
            .map(|result| result.rows_affected())
            .map_err(operation_failed)
    }
}

impl SqlxExternalEmailOutbox<'_> {
    async fn existing_message_id(
        &mut self,
        origin: ExternalEmailOrigin,
        template: &str,
    ) -> Result<ExternalEmailMessageId, ExternalEmailOutboxError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            "SELECT external_email_message_id FROM external_email_messages WHERE origin_type = $1 AND origin_id = $2 AND template = $3",
        )
        .bind(origin.origin_type())
        .bind(origin.origin_id())
        .bind(template)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(operation_failed)?;
        ExternalEmailMessageId::try_from(id).map_err(|source| {
            ExternalEmailOutboxError::InvalidPersistedState {
                source: box_error(source),
            }
        })
    }
}

fn operation_failed(source: sqlx::Error) -> ExternalEmailOutboxError {
    ExternalEmailOutboxError::OperationFailed {
        source: box_error(source),
    }
}
