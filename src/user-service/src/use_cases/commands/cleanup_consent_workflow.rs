use crate::ports::{
    NewsletterConfirmationChallengeError, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationClock,
    consent_workflow_cleanup::{
        ConsentWorkflowCleanup, ConsentWorkflowCleanupError, ConsentWorkflowCleanupFactory,
    },
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, UnitOfWork};
use std::fmt;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum CleanupConsentWorkflowError {
    #[error("transaction failed")]
    TransactionFailed(#[source] BoxError),
    #[error("cleanup batch size must be between 1 and 1000")]
    InvalidBatchSize,
    #[error("consent workflow cleanup of {target} unavailable")]
    TemporarilyUnavailable {
        target: ConsentWorkflowCleanupTarget,
        #[source]
        source: BoxError,
    },
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

/// The record set one bounded cleanup batch deletes from, each in its own transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentWorkflowCleanupTarget {
    UnconfirmedChallenges,
    ConfirmedChallenges,
    CompletedIntents,
    WebhookReceipts,
}

impl ConsentWorkflowCleanupTarget {
    const fn label(&self) -> &'static str {
        match self {
            Self::UnconfirmedChallenges => "unconfirmed_challenges",
            Self::ConfirmedChallenges => "confirmed_challenges",
            Self::CompletedIntents => "completed_intents",
            Self::WebhookReceipts => "webhook_receipts",
        }
    }
}

impl fmt::Display for ConsentWorkflowCleanupTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
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
        let unconfirmed_challenges_deleted = self
            .cleanup_target(
                now,
                batch_size,
                ConsentWorkflowCleanupTarget::UnconfirmedChallenges,
            )
            .await?;
        let confirmed_challenges_deleted = self
            .cleanup_target(
                now,
                batch_size,
                ConsentWorkflowCleanupTarget::ConfirmedChallenges,
            )
            .await?;
        let completed_intents_deleted = self
            .cleanup_target(
                now,
                batch_size,
                ConsentWorkflowCleanupTarget::CompletedIntents,
            )
            .await?;
        let webhook_receipts_deleted = self
            .cleanup_target(
                now,
                batch_size,
                ConsentWorkflowCleanupTarget::WebhookReceipts,
            )
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
    C: NewsletterConfirmationChallengesFactory<U::Tx>,
    M: ConsentWorkflowCleanupFactory<U::Tx>,
{
    async fn begin(&self) -> Result<U::Tx, CleanupConsentWorkflowError> {
        self.unit_of_work
            .begin()
            .await
            .map_err(|source| CleanupConsentWorkflowError::TransactionFailed(Box::new(source)))
    }

    /// Deletes one bounded batch of `target` in its own transaction, so a later target's
    /// failure never rolls back batches that already committed.
    async fn cleanup_target(
        &self,
        now: OffsetDateTime,
        batch_size: u16,
        target: ConsentWorkflowCleanupTarget,
    ) -> Result<u64, CleanupConsentWorkflowError> {
        let mut tx = self.begin().await?;
        let deleted = self.delete_batch(&mut tx, now, batch_size, target).await?;
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

    async fn delete_batch(
        &self,
        tx: &mut U::Tx,
        now: OffsetDateTime,
        batch_size: u16,
        target: ConsentWorkflowCleanupTarget,
    ) -> Result<u64, CleanupConsentWorkflowError> {
        let deleted = match target {
            ConsentWorkflowCleanupTarget::UnconfirmedChallenges => {
                return self
                    .challenges
                    .in_transaction(tx)
                    .cleanup_expired(now, batch_size)
                    .await
                    .map_err(challenge_cleanup_error);
            }
            ConsentWorkflowCleanupTarget::ConfirmedChallenges => {
                let mut cleanup = self.maintenance.in_transaction(tx);
                cleanup.cleanup_confirmed_challenges(now, batch_size).await
            }
            ConsentWorkflowCleanupTarget::CompletedIntents => {
                let mut cleanup = self.maintenance.in_transaction(tx);
                cleanup.cleanup_completed_intents(now, batch_size).await
            }
            ConsentWorkflowCleanupTarget::WebhookReceipts => {
                let mut cleanup = self.maintenance.in_transaction(tx);
                cleanup.cleanup_webhook_receipts(now, batch_size).await
            }
        };
        deleted.map_err(|error| maintenance_cleanup_error(target, error))
    }
}

fn challenge_cleanup_error(
    error: NewsletterConfirmationChallengeError,
) -> CleanupConsentWorkflowError {
    let target = ConsentWorkflowCleanupTarget::UnconfirmedChallenges;
    match error {
        NewsletterConfirmationChallengeError::InvalidInput => {
            CleanupConsentWorkflowError::InvalidBatchSize
        }
        NewsletterConfirmationChallengeError::TemporarilyUnavailable { source } => {
            CleanupConsentWorkflowError::TemporarilyUnavailable { target, source }
        }
        error @ NewsletterConfirmationChallengeError::InvalidPersistedState => {
            CleanupConsentWorkflowError::TemporarilyUnavailable {
                target,
                source: box_error(error),
            }
        }
    }
}

fn maintenance_cleanup_error(
    target: ConsentWorkflowCleanupTarget,
    error: ConsentWorkflowCleanupError,
) -> CleanupConsentWorkflowError {
    match error {
        ConsentWorkflowCleanupError::InvalidBatchSize => {
            CleanupConsentWorkflowError::InvalidBatchSize
        }
        ConsentWorkflowCleanupError::TemporarilyUnavailable { source } => {
            CleanupConsentWorkflowError::TemporarilyUnavailable { target, source }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        NewNewsletterConfirmationChallenge, NewsletterConfirmationChallenge,
        NewsletterConfirmationIssueOutcome, NewsletterConfirmationSendStatus,
    };
    use application::error::static_error;
    use application::transaction::TransactionError;
    use serde_email::Email;
    use std::sync::{Arc, Mutex};
    use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
    use user_core::newsletter_confirmation::NewsletterConfirmationTokenDigest;
    use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
    use user_core::user_id::UserId;

    const TARGETS: [ConsentWorkflowCleanupTarget; 4] = [
        ConsentWorkflowCleanupTarget::UnconfirmedChallenges,
        ConsentWorkflowCleanupTarget::ConfirmedChallenges,
        ConsentWorkflowCleanupTarget::CompletedIntents,
        ConsentWorkflowCleanupTarget::WebhookReceipts,
    ];

    #[derive(Clone, Copy)]
    enum Failure {
        Unavailable,
        BatchRejected,
    }

    struct State {
        failing: ConsentWorkflowCleanupTarget,
        failure: Failure,
        commits: usize,
    }

    #[derive(Clone)]
    struct Fake(Arc<Mutex<State>>);
    struct FakeTx(Arc<Mutex<State>>);
    struct FakeChallenges(Arc<Mutex<State>>);
    struct FakeMaintenance(Arc<Mutex<State>>);

    impl Fake {
        fn failing(failing: ConsentWorkflowCleanupTarget, failure: Failure) -> Self {
            Self(Arc::new(Mutex::new(State {
                failing,
                failure,
                commits: 0,
            })))
        }

        fn commits(&self) -> usize {
            self.0.lock().unwrap().commits
        }
    }

    fn handler(fake: &Fake) -> CleanupConsentWorkflowHandler<Fake, Fake, Fake, Fake> {
        CleanupConsentWorkflowHandler::new(fake.clone(), fake.clone(), fake.clone(), fake.clone())
    }

    /// Returns `Some(failure)` when `target` is the configured failing target.
    fn failure_for(
        state: &Arc<Mutex<State>>,
        target: ConsentWorkflowCleanupTarget,
    ) -> Option<Failure> {
        let state = state.lock().unwrap();
        (state.failing == target).then_some(state.failure)
    }

    fn maintenance_result(
        state: &Arc<Mutex<State>>,
        target: ConsentWorkflowCleanupTarget,
    ) -> Result<u64, ConsentWorkflowCleanupError> {
        match failure_for(state, target) {
            None => Ok(1),
            Some(Failure::BatchRejected) => Err(ConsentWorkflowCleanupError::InvalidBatchSize),
            Some(Failure::Unavailable) => {
                Err(ConsentWorkflowCleanupError::TemporarilyUnavailable {
                    source: static_error("fixture delete rejected"),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            self.0.lock().unwrap().commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fake {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<FakeTx, TransactionError> {
            Ok(FakeTx(self.0.clone()))
        }
    }

    impl NewsletterConfirmationClock for Fake {
        fn now_utc(&self) -> OffsetDateTime {
            OffsetDateTime::UNIX_EPOCH
        }
    }

    impl NewsletterConfirmationChallengesFactory<FakeTx> for Fake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl NewsletterConfirmationChallenges + 'tx {
            FakeChallenges(self.0.clone())
        }
    }

    impl ConsentWorkflowCleanupFactory<FakeTx> for Fake {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ConsentWorkflowCleanup + 'tx {
            FakeMaintenance(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl ConsentWorkflowCleanup for FakeMaintenance {
        async fn cleanup_confirmed_challenges(
            &mut self,
            _now: OffsetDateTime,
            _batch_size: u16,
        ) -> Result<u64, ConsentWorkflowCleanupError> {
            maintenance_result(&self.0, ConsentWorkflowCleanupTarget::ConfirmedChallenges)
        }

        async fn cleanup_completed_intents(
            &mut self,
            _now: OffsetDateTime,
            _batch_size: u16,
        ) -> Result<u64, ConsentWorkflowCleanupError> {
            maintenance_result(&self.0, ConsentWorkflowCleanupTarget::CompletedIntents)
        }

        async fn cleanup_webhook_receipts(
            &mut self,
            _now: OffsetDateTime,
            _batch_size: u16,
        ) -> Result<u64, ConsentWorkflowCleanupError> {
            maintenance_result(&self.0, ConsentWorkflowCleanupTarget::WebhookReceipts)
        }
    }

    #[async_trait::async_trait]
    impl NewsletterConfirmationChallenges for FakeChallenges {
        async fn create_if_allowed(
            &mut self,
            _: NewNewsletterConfirmationChallenge,
        ) -> Result<NewsletterConfirmationIssueOutcome, NewsletterConfirmationChallengeError>
        {
            unreachable!()
        }

        async fn record_send_outcome(
            &mut self,
            _: NewsletterConfirmationId,
            _: NewsletterConfirmationSendStatus,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn find_by_token_digest(
            &mut self,
            _: NewsletterConfirmationTokenDigest,
        ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>
        {
            unreachable!()
        }

        async fn lock_for_confirmation(
            &mut self,
            _: NewsletterConfirmationId,
            _: NewsletterConfirmationTokenDigest,
        ) -> Result<Option<NewsletterConfirmationChallenge>, NewsletterConfirmationChallengeError>
        {
            unreachable!()
        }

        async fn invalidate(
            &mut self,
            _: NewsletterConfirmationId,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn complete_confirmation(
            &mut self,
            _: NewsletterConfirmationId,
            _: Option<UserId>,
            _: MarketingConsentSyncIntentId,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn invalidate_pending_for_email(
            &mut self,
            _: &Email,
            _: OffsetDateTime,
        ) -> Result<(), NewsletterConfirmationChallengeError> {
            unreachable!()
        }

        async fn cleanup_expired(
            &mut self,
            _now: OffsetDateTime,
            _batch_size: u16,
        ) -> Result<u64, NewsletterConfirmationChallengeError> {
            match failure_for(&self.0, ConsentWorkflowCleanupTarget::UnconfirmedChallenges) {
                None => Ok(1),
                Some(Failure::BatchRejected) => {
                    Err(NewsletterConfirmationChallengeError::InvalidInput)
                }
                Some(Failure::Unavailable) => Err(
                    NewsletterConfirmationChallengeError::TemporarilyUnavailable {
                        source: static_error("fixture delete rejected"),
                    },
                ),
            }
        }
    }

    #[tokio::test]
    async fn should_keep_cause_and_target_and_skip_commit_when_a_cleanup_batch_fails() {
        for (committed_before_failure, target) in TARGETS.into_iter().enumerate() {
            let fake = Fake::failing(target, Failure::Unavailable);

            let result = handler(&fake).execute(10).await;

            let Err(CleanupConsentWorkflowError::TemporarilyUnavailable {
                target: failed_target,
                source,
            }) = result
            else {
                panic!("a failed {target} batch must make the cleanup unavailable");
            };
            assert_eq!(target, failed_target);
            assert_eq!("fixture delete rejected", source.to_string());
            assert_eq!(committed_before_failure, fake.commits());
        }
    }

    #[tokio::test]
    async fn should_report_adapter_batch_rejection_as_invalid_batch_size() {
        for target in TARGETS {
            let fake = Fake::failing(target, Failure::BatchRejected);

            let result = handler(&fake).execute(10).await;

            assert!(matches!(
                result,
                Err(CleanupConsentWorkflowError::InvalidBatchSize)
            ));
        }
    }
}
