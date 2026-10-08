use application::error::box_error;
use platform_postgres::SqlxTransaction;
use serde_email::Email;
use sqlx::FromRow;
use time::OffsetDateTime;
use user_service::ports::{
    LoopsPreferenceFence, LoopsWebhookReceiptError, LoopsWebhookReceiptInput,
    LoopsWebhookReceiptLookup, LoopsWebhookReceiptWriteOutcome, LoopsWebhookReceipts,
    LoopsWebhookReceiptsFactory,
};

#[derive(Clone, Copy, Default)]
pub struct SqlxLoopsWebhookReceiptRepository;

impl SqlxLoopsWebhookReceiptRepository {
    pub fn new() -> Self {
        Self
    }
}

struct SqlxLoopsWebhookReceipts<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl LoopsWebhookReceiptsFactory<SqlxTransaction> for SqlxLoopsWebhookReceiptRepository {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl LoopsWebhookReceipts + 'tx {
        SqlxLoopsWebhookReceipts { tx }
    }
}

#[derive(FromRow)]
struct PreferenceFenceRow {
    latest_event_at: OffsetDateTime,
    provider_contact_id: String,
    purpose_subscribed: bool,
}

impl TryFrom<PreferenceFenceRow> for LoopsPreferenceFence {
    type Error = LoopsWebhookReceiptError;

    fn try_from(row: PreferenceFenceRow) -> Result<Self, Self::Error> {
        if row.provider_contact_id.is_empty()
            || row.provider_contact_id.len() > 256
            || row.provider_contact_id.chars().any(char::is_control)
        {
            return Err(LoopsWebhookReceiptError::InvalidPersistedState);
        }
        Ok(Self {
            latest_event_at: row.latest_event_at,
            provider_contact_id: row.provider_contact_id,
            purpose_subscribed: row.purpose_subscribed,
        })
    }
}

#[async_trait::async_trait]
impl LoopsWebhookReceipts for SqlxLoopsWebhookReceipts<'_> {
    async fn find_by_delivery_id(
        &mut self,
        delivery_id: &str,
    ) -> Result<Option<LoopsWebhookReceiptLookup>, LoopsWebhookReceiptError> {
        let digest: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT COALESCE(\
                (SELECT raw_body_sha256 FROM loops_webhook_receipts WHERE delivery_id = $1), \
                (SELECT raw_body_sha256 FROM loops_webhook_positive_delivery_tombstones WHERE delivery_id = $1))",
        )
        .bind(delivery_id)
        .fetch_one(self.tx.connection())
        .await
        .map_err(|source| LoopsWebhookReceiptError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        digest
            .map(|digest| {
                let raw_body_sha256: [u8; 32] = digest
                    .try_into()
                    .map_err(|_| LoopsWebhookReceiptError::InvalidPersistedState)?;
                Ok(LoopsWebhookReceiptLookup { raw_body_sha256 })
            })
            .transpose()
    }

    async fn find_preference_fence(
        &mut self,
        email: &Email,
    ) -> Result<Option<LoopsPreferenceFence>, LoopsWebhookReceiptError> {
        sqlx::query_as::<_, PreferenceFenceRow>(
            "SELECT latest_event_at, provider_contact_id, purpose_subscribed FROM loops_webhook_preference_fences WHERE email = $1",
        )
        .bind::<&str>(email.as_ref())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|source| LoopsWebhookReceiptError::TemporarilyUnavailable {
            source: box_error(source),
        })?
        .map(LoopsPreferenceFence::try_from)
        .transpose()
    }

    async fn advance_preference_fence(
        &mut self,
        email: &Email,
        provider_contact_id: &str,
        event_at: OffsetDateTime,
        purpose_subscribed: bool,
    ) -> Result<(), LoopsWebhookReceiptError> {
        let result = sqlx::query(
            "INSERT INTO loops_webhook_preference_fences (email, latest_event_at, provider_contact_id, purpose_subscribed) VALUES ($1, $2, $3, $4) ON CONFLICT (email) DO UPDATE SET latest_event_at = EXCLUDED.latest_event_at, provider_contact_id = EXCLUDED.provider_contact_id, purpose_subscribed = EXCLUDED.purpose_subscribed, updated_at = clock_timestamp() WHERE EXCLUDED.latest_event_at > loops_webhook_preference_fences.latest_event_at OR (EXCLUDED.latest_event_at = loops_webhook_preference_fences.latest_event_at AND EXCLUDED.purpose_subscribed = false)",
        )
        .bind::<&str>(email.as_ref())
        .bind(event_at)
        .bind(provider_contact_id)
        .bind(purpose_subscribed)
        .execute(self.tx.connection())
        .await
        .map_err(|source| LoopsWebhookReceiptError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(LoopsWebhookReceiptError::ConcurrencyConflict)
        }
    }

    async fn insert_receipt(
        &mut self,
        receipt: LoopsWebhookReceiptInput,
    ) -> Result<LoopsWebhookReceiptWriteOutcome, LoopsWebhookReceiptError> {
        // Early ignored positives do not take a mailbox lock. Check the durable
        // marker here too, before inserting a fresh receipt after cleanup.
        if receipt.event_name == "email.resubscribed"
            && let Some(existing) = self.find_by_delivery_id(&receipt.delivery_id).await?
        {
            return Ok(if existing.raw_body_sha256 == receipt.raw_body_sha256 {
                LoopsWebhookReceiptWriteOutcome::ExistingSameDigest
            } else {
                LoopsWebhookReceiptWriteOutcome::ExistingDifferentDigest
            });
        }
        let result = sqlx::query(
            "INSERT INTO loops_webhook_receipts (delivery_id, raw_body_sha256, provider_event_name, event_time, email, provider_contact_id, mailing_list_id, disposition, processed_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) ON CONFLICT (delivery_id) DO NOTHING",
        )
        .bind(&receipt.delivery_id)
        .bind(receipt.raw_body_sha256.as_slice())
        .bind(&receipt.event_name)
        .bind(receipt.event_time)
        .bind(&receipt.email)
        .bind(&receipt.provider_contact_id)
        .bind(&receipt.mailing_list_id)
        .bind(receipt.disposition.as_str())
        .bind(receipt.processed_at)
        .bind(receipt.expires_at)
        .execute(self.tx.connection())
        .await
        .map_err(|source| LoopsWebhookReceiptError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        if result.rows_affected() == 1 {
            return Ok(LoopsWebhookReceiptWriteOutcome::Inserted);
        }

        let existing = self
            .find_by_delivery_id(&receipt.delivery_id)
            .await?
            .ok_or(LoopsWebhookReceiptError::ConcurrencyConflict)?;
        if existing.raw_body_sha256 == receipt.raw_body_sha256 {
            Ok(LoopsWebhookReceiptWriteOutcome::ExistingSameDigest)
        } else {
            Ok(LoopsWebhookReceiptWriteOutcome::ExistingDifferentDigest)
        }
    }
}
