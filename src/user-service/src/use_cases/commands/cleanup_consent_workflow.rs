use crate::ports::{
    NewsletterConfirmationChallenges, NewsletterConfirmationChallengesFactory,
    NewsletterConfirmationClock,
    consent_workflow_cleanup::{ConsentWorkflowCleanup, ConsentWorkflowCleanupFactory},
};
use application::transaction::{Transaction, UnitOfWork};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum CleanupConsentWorkflowError {
    #[error("transaction failed")]
    TransactionFailed(#[source] application::error::BoxError),
    #[error("cleanup batch size must be between 1 and 1000")]
    InvalidBatchSize,
    #[error("consent workflow cleanup unavailable")]
    TemporarilyUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsentWorkflowCleanupCounts {
    pub unconfirmed_challenges_deleted: u64,
    pub confirmed_challenges_deleted: u64,
    pub completed_intents_deleted: u64,
    pub webhook_receipts_deleted: u64,
}

#[async_trait::async_trait]
pub trait CleanupConsentWorkflowUseCase: Send + Sync {
    async fn execute(
        &self,
        batch_size: u16,
    ) -> Result<ConsentWorkflowCleanupCounts, CleanupConsentWorkflowError>;
}

pub struct CleanupConsentWorkflowHandler<U, C, M, K> {
    unit_of_work: U,
    challenges: C,
    maintenance: M,
    clock: K,
}

#[derive(Clone, Copy)]
enum CleanupTarget {
    ConfirmedChallenges,
    CompletedIntents,
    WebhookReceipts,
}

impl CleanupTarget {
    const fn label(&self) -> &'static str {
        match self {
            Self::ConfirmedChallenges => "confirmed_challenges",
            Self::CompletedIntents => "completed_intents",
            Self::WebhookReceipts => "webhook_receipts",
        }
    }
}

impl<U, C, M, K> CleanupConsentWorkflowHandler<U, C, M, K> {
    pub fn new(unit_of_work: U, challenges: C, maintenance: M, clock: K) -> Self {
        Self {
            unit_of_work,
            challenges,
            maintenance,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, M, K> CleanupConsentWorkflowUseCase for CleanupConsentWorkflowHandler<U, C, M, K>
where
    U: UnitOfWork,
    C: NewsletterConfirmationChallengesFactory<U::Tx>,
    M: ConsentWorkflowCleanupFactory<U::Tx>,
    K: NewsletterConfirmationClock,
{
    async fn execute(
        &self,
        batch_size: u16,
    ) -> Result<ConsentWorkflowCleanupCounts, CleanupConsentWorkflowError> {
        if !(1..=1_000).contains(&batch_size) {
            return Err(CleanupConsentWorkflowError::InvalidBatchSize);
        }
        let now = self.clock.now_utc();
        let mut tx = self.begin().await?;
        let unconfirmed_challenges_deleted = self
            .challenges
            .in_transaction(&mut tx)
            .cleanup_expired(now, batch_size)
            .await
            .map_err(|_| CleanupConsentWorkflowError::TemporarilyUnavailable)?;
        tx.commit()
            .await
            .map_err(|source| CleanupConsentWorkflowError::TransactionFailed(Box::new(source)))?;
        tracing::info!(
            event = "consent_workflow.cleanup_target",
            target = "unconfirmed_challenges",
            outcome = "success",
            deleted = unconfirmed_challenges_deleted,
        );

        let confirmed_challenges_deleted = self
            .run_maintenance(now, batch_size, CleanupTarget::ConfirmedChallenges)
            .await?;
        let completed_intents_deleted = self
            .run_maintenance(now, batch_size, CleanupTarget::CompletedIntents)
            .await?;
        let webhook_receipts_deleted = self
            .run_maintenance(now, batch_size, CleanupTarget::WebhookReceipts)
            .await?;
        Ok(ConsentWorkflowCleanupCounts {
            unconfirmed_challenges_deleted,
            confirmed_challenges_deleted,
            completed_intents_deleted,
            webhook_receipts_deleted,
        })
    }
}

impl<U, C, M, K> CleanupConsentWorkflowHandler<U, C, M, K>
where
    U: UnitOfWork,
    M: ConsentWorkflowCleanupFactory<U::Tx>,
{
    async fn begin(&self) -> Result<U::Tx, CleanupConsentWorkflowError> {
        self.unit_of_work
            .begin()
            .await
            .map_err(|source| CleanupConsentWorkflowError::TransactionFailed(Box::new(source)))
    }

    async fn run_maintenance(
        &self,
        now: OffsetDateTime,
        batch_size: u16,
        target: CleanupTarget,
    ) -> Result<u64, CleanupConsentWorkflowError> {
        let mut tx = self.begin().await?;
        let mut cleanup = self.maintenance.in_transaction(&mut tx);
        let deleted = match target {
            CleanupTarget::ConfirmedChallenges => {
                cleanup.cleanup_confirmed_challenges(now, batch_size).await
            }
            CleanupTarget::CompletedIntents => {
                cleanup.cleanup_completed_intents(now, batch_size).await
            }
            CleanupTarget::WebhookReceipts => {
                cleanup.cleanup_webhook_receipts(now, batch_size).await
            }
        }
        .map_err(|_| CleanupConsentWorkflowError::TemporarilyUnavailable)?;
        drop(cleanup);
        tx.commit()
            .await
            .map_err(|source| CleanupConsentWorkflowError::TransactionFailed(Box::new(source)))?;
        tracing::info!(
            event = "consent_workflow.cleanup_target",
            target = target.label(),
            outcome = "success",
            deleted,
        );
        Ok(deleted)
    }
}
