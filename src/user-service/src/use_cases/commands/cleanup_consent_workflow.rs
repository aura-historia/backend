use crate::ports::{
    NewsletterConfirmationChallengeError, NewsletterConfirmationChallenges,
    NewsletterConfirmationChallengesFactory, NewsletterConfirmationClock,
    consent_workflow_cleanup::{
        ConsentWorkflowCleanup, ConsentWorkflowCleanupError, ConsentWorkflowCleanupFactory,
    },
};
use application::error::{BoxError, box_error};
use application::transaction::{Transaction, TransactionError, UnitOfWork};
use std::fmt;
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum CleanupConsentWorkflowError {
    #[error("consent workflow cleanup transaction for {target} failed")]
    TransactionFailed {
        target: ConsentWorkflowCleanupTarget,
        #[source]
        source: BoxError,
    },
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
    pub const fn label(&self) -> &'static str {
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
        use ConsentWorkflowCleanupTarget as Target;

        if !(1..=1_000).contains(&batch_size) {
            return Err(CleanupConsentWorkflowError::InvalidBatchSize);
        }
        let now = self.clock.now_utc();
        let unconfirmed_challenges_deleted = self
            .cleanup_target(now, batch_size, Target::UnconfirmedChallenges)
            .await?;
        let confirmed_challenges_deleted = self
            .cleanup_target(now, batch_size, Target::ConfirmedChallenges)
            .await?;
        let completed_intents_deleted = self
            .cleanup_target(now, batch_size, Target::CompletedIntents)
            .await?;
        let webhook_receipts_deleted = self
            .cleanup_target(now, batch_size, Target::WebhookReceipts)
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
    /// Deletes one bounded batch of `target` in its own transaction, so a later target's
    /// failure never rolls back batches that already committed.
    async fn cleanup_target(
        &self,
        now: OffsetDateTime,
        batch_size: u16,
        target: ConsentWorkflowCleanupTarget,
    ) -> Result<u64, CleanupConsentWorkflowError> {
        let transaction_failed =
            |source: TransactionError| CleanupConsentWorkflowError::TransactionFailed {
                target,
                source: box_error(source),
            };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(transaction_failed)?;
        let deleted = self.delete_batch(&mut tx, now, batch_size, target).await?;
        tx.commit().await.map_err(transaction_failed)?;
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
        let maintenance_failed =
            |error: ConsentWorkflowCleanupError| maintenance_cleanup_error(target, error);
        match target {
            ConsentWorkflowCleanupTarget::UnconfirmedChallenges => self
                .challenges
                .in_transaction(tx)
                .cleanup_expired(now, batch_size)
                .await
                .map_err(challenge_cleanup_error),
            ConsentWorkflowCleanupTarget::ConfirmedChallenges => self
                .maintenance
                .in_transaction(tx)
                .cleanup_confirmed_challenges(now, batch_size)
                .await
                .map_err(maintenance_failed),
            ConsentWorkflowCleanupTarget::CompletedIntents => self
                .maintenance
                .in_transaction(tx)
                .cleanup_completed_intents(now, batch_size)
                .await
                .map_err(maintenance_failed),
            ConsentWorkflowCleanupTarget::WebhookReceipts => self
                .maintenance
                .in_transaction(tx)
                .cleanup_webhook_receipts(now, batch_size)
                .await
                .map_err(maintenance_failed),
        }
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
        // Expiry cleanup reads no persisted state; should the adapter ever report it, keep that
        // report as the cause instead of inventing a narrower outcome.
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
    use serde_email::Email;
    use std::sync::{Arc, Mutex};
    use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
    use user_core::newsletter_confirmation::NewsletterConfirmationTokenDigest;
    use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
    use user_core::user_id::UserId;

    /// In execution order: the index of a target equals the commits made before it runs.
    const TARGETS: [ConsentWorkflowCleanupTarget; 4] = [
        ConsentWorkflowCleanupTarget::UnconfirmedChallenges,
        ConsentWorkflowCleanupTarget::ConfirmedChallenges,
        ConsentWorkflowCleanupTarget::CompletedIntents,
        ConsentWorkflowCleanupTarget::WebhookReceipts,
    ];

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Failure {
        Unavailable,
        BatchRejected,
        CommitRejected,
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
            None | Some(Failure::CommitRejected) => Ok(1),
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
            let mut state = self.0.lock().unwrap();
            if state.failure == Failure::CommitRejected && TARGETS[state.commits] == state.failing {
                return Err(TransactionError::CommitFailed(static_error(
                    "fixture commit rejected",
                )));
            }
            state.commits += 1;
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
                None | Some(Failure::CommitRejected) => Ok(1),
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

    fn root_cause(error: &CleanupConsentWorkflowError) -> String {
        let mut cause: &dyn std::error::Error = error;
        while let Some(source) = cause.source() {
            cause = source;
        }
        cause.to_string()
    }

    #[tokio::test]
    async fn should_keep_cause_and_target_and_skip_commit_when_a_cleanup_batch_fails() {
        for (committed_before_failure, target) in TARGETS.into_iter().enumerate() {
            let fake = Fake::failing(target, Failure::Unavailable);

            let Err(error) = handler(&fake).execute(10).await else {
                panic!("a failed {target} batch must fail the cleanup");
            };

            assert!(matches!(
                error,
                CleanupConsentWorkflowError::TemporarilyUnavailable { target: failed, .. }
                    if failed == target
            ));
            assert_eq!("fixture delete rejected", root_cause(&error));
            assert_eq!(committed_before_failure, fake.commits());
        }
    }

    #[tokio::test]
    async fn should_keep_cause_and_target_when_a_cleanup_commit_fails() {
        for (committed_before_failure, target) in TARGETS.into_iter().enumerate() {
            let fake = Fake::failing(target, Failure::CommitRejected);

            let Err(error) = handler(&fake).execute(10).await else {
                panic!("a failed {target} commit must fail the cleanup");
            };

            assert!(matches!(
                error,
                CleanupConsentWorkflowError::TransactionFailed { target: failed, .. }
                    if failed == target
            ));
            assert_eq!("fixture commit rejected", root_cause(&error));
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
