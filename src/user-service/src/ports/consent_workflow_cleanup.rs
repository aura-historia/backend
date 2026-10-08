use time::OffsetDateTime;

pub const CONFIRMED_CHALLENGE_REPLAY_DAYS: i64 = 7;
pub const CONSENT_WORKFLOW_RECEIPT_RETENTION_DAYS: i64 = 120;

/// Operational retention only. A missing receipt never authorizes a new grant.
#[derive(Debug, thiserror::Error)]
#[error("consent workflow cleanup unavailable")]
pub struct ConsentWorkflowCleanupError;

#[async_trait::async_trait]
pub trait ConsentWorkflowCleanup: Send {
    async fn cleanup_confirmed_challenges(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError>;

    async fn cleanup_completed_intents(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError>;

    async fn cleanup_webhook_receipts(
        &mut self,
        now: OffsetDateTime,
        batch_size: u16,
    ) -> Result<u64, ConsentWorkflowCleanupError>;
}

pub trait ConsentWorkflowCleanupFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ConsentWorkflowCleanup + 'tx;
}
