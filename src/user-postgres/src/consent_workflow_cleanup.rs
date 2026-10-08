use platform_postgres::SqlxTransaction;
use time::{Duration, OffsetDateTime};
use user_service::ports::consent_workflow_cleanup::{
    CONFIRMED_CHALLENGE_REPLAY_DAYS, CONSENT_WORKFLOW_RECEIPT_RETENTION_DAYS,
    ConsentWorkflowCleanup, ConsentWorkflowCleanupError, ConsentWorkflowCleanupFactory,
};

#[derive(Clone, Copy, Default)]
pub struct SqlxConsentWorkflowCleanup;

impl SqlxConsentWorkflowCleanup {
    pub fn new() -> Self {
        Self
    }
}

struct SqlxConsentWorkflowCleanupTransaction<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl ConsentWorkflowCleanupFactory<SqlxTransaction> for SqlxConsentWorkflowCleanup {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ConsentWorkflowCleanup + 'tx {
        SqlxConsentWorkflowCleanupTransaction { tx }
    }
}

#[async_trait::async_trait]
impl ConsentWorkflowCleanup for SqlxConsentWorkflowCleanupTransaction<'_> {
    async fn cleanup_confirmed_challenges(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError> {
        validate_batch_size(batch_size)?;
        // The issuance window must also close, even if confirmation happened earlier.
        let result = sqlx::query(
            "WITH eligible AS (SELECT confirmation_id FROM newsletter_subscription_confirmations \
             WHERE confirmed_at <= $2 AND expires_at <= $1 \
             ORDER BY confirmed_at, confirmation_id LIMIT $3 FOR UPDATE SKIP LOCKED) \
             DELETE FROM newsletter_subscription_confirmations AS challenge USING eligible \
             WHERE challenge.confirmation_id = eligible.confirmation_id",
        )
        .bind(now)
        .bind(now - Duration::days(CONFIRMED_CHALLENGE_REPLAY_DAYS))
        .bind(i64::from(batch_size))
        .execute(self.tx.connection())
        .await
        .map_err(|_| ConsentWorkflowCleanupError)?;
        Ok(result.rows_affected())
    }

    async fn cleanup_completed_intents(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError> {
        validate_batch_size(batch_size)?;
        let result = sqlx::query(
            "WITH eligible AS (SELECT intent_id FROM marketing_email_consent_sync_intents \
             WHERE status IN ('APPLIED', 'SUPERSEDED') AND completed_at <= $1 \
             AND NOT EXISTS (SELECT 1 FROM newsletter_subscription_confirmations AS proof \
                             WHERE proof.resulting_intent_id = marketing_email_consent_sync_intents.intent_id) \
             ORDER BY completed_at, intent_id LIMIT $2 FOR UPDATE SKIP LOCKED) \
             DELETE FROM marketing_email_consent_sync_intents AS intent USING eligible \
             WHERE intent.intent_id = eligible.intent_id",
        )
        .bind(now - Duration::days(CONSENT_WORKFLOW_RECEIPT_RETENTION_DAYS))
        .bind(i64::from(batch_size))
        .execute(self.tx.connection())
        .await
        .map_err(|_| ConsentWorkflowCleanupError)?;
        Ok(result.rows_affected())
    }

    async fn cleanup_webhook_receipts(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError> {
        validate_batch_size(batch_size)?;
        // processed_at protects receipts written before the 120-day policy even
        // if their stored expires_at used the earlier 35-day window.
        let result = sqlx::query(
            "WITH eligible AS (SELECT delivery_id FROM loops_webhook_receipts \
             WHERE processed_at <= $1 AND expires_at <= $2 \
             ORDER BY processed_at, delivery_id LIMIT $3 FOR UPDATE SKIP LOCKED) \
             DELETE FROM loops_webhook_receipts AS receipt USING eligible \
             WHERE receipt.delivery_id = eligible.delivery_id",
        )
        .bind(now - Duration::days(CONSENT_WORKFLOW_RECEIPT_RETENTION_DAYS))
        .bind(now)
        .bind(i64::from(batch_size))
        .execute(self.tx.connection())
        .await
        .map_err(|_| ConsentWorkflowCleanupError)?;
        Ok(result.rows_affected())
    }
}

fn validate_batch_size(batch_size: u16) -> Result<(), ConsentWorkflowCleanupError> {
    if (1..=1_000).contains(&batch_size) {
        Ok(())
    } else {
        Err(ConsentWorkflowCleanupError)
    }
}
