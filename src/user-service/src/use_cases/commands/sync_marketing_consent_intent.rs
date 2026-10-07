use crate::{
    ports::{
        ConsentIntent, ConsentIntentSource, ConsentSubject, GrantRaceRepairOutcome,
        MarketingConsentIntentError, MarketingConsentIntentsFactory, MarketingEmailConsentError,
        MarketingEmailConsentOutcome, MarketingEmailConsentWriter, MarketingEmailSubscriptionState,
        marketing_consent_intents::{
            ConsentWorkerClaim, ConsentWorkerClaimOutcome, ConsentWorkerFinalization,
            ConsentWorkerRecheckOutcome, ConsentWorkerTerminalStatus, MarketingConsentIntentWorker,
        },
    },
    use_cases::commands::coordinate_marketing_consent::{
        CoordinateMarketingConsentError, MarketingConsentCoordinator,
    },
};
use application::transaction::{Transaction, UnitOfWork};
use time::OffsetDateTime;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMarketingConsentIntentResult {
    Applied,
    Blocked,
    Superseded,
    AlreadyTerminal,
    Deferred,
    Retryable,
    Missing,
}

impl SyncMarketingConsentIntentResult {
    pub const fn is_complete(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Blocked | Self::Superseded | Self::AlreadyTerminal
        )
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SyncMarketingConsentIntentError {
    #[error("consent intent persistence unavailable")]
    Persistence,
    #[error("consent intent transaction outcome is unconfirmed")]
    Transaction,
    #[error("invalid persisted consent intent")]
    InvalidPersistedState,
}

#[async_trait::async_trait]
pub trait SyncMarketingConsentIntentUseCase: Send + Sync {
    async fn execute(
        &self,
        intent_id: MarketingConsentSyncIntentId,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError>;
}

pub struct SyncMarketingConsentIntentHandler<U, W, C, P> {
    unit_of_work: U,
    worker: W,
    intents: C,
    provider: P,
}

impl<U, W, C, P> SyncMarketingConsentIntentHandler<U, W, C, P> {
    pub fn new(unit_of_work: U, worker: W, intents: C, provider: P) -> Self {
        Self {
            unit_of_work,
            worker,
            intents,
            provider,
        }
    }
}

#[async_trait::async_trait]
impl<U, W, C, P> SyncMarketingConsentIntentUseCase for SyncMarketingConsentIntentHandler<U, W, C, P>
where
    U: UnitOfWork,
    W: MarketingConsentIntentWorker<U::Tx>,
    C: MarketingConsentIntentsFactory<U::Tx>,
    P: MarketingEmailConsentWriter,
{
    async fn execute(
        &self,
        intent_id: MarketingConsentSyncIntentId,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
        let claim = self
            .worker
            .claim_by_id(&mut tx, intent_id)
            .await
            .map_err(map_persistence_error)?;
        match claim {
            ConsentWorkerClaimOutcome::Claimed(claim) => {
                tx.commit()
                    .await
                    .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                self.execute_claim(claim).await
            }
            ConsentWorkerClaimOutcome::Missing => {
                tx.commit()
                    .await
                    .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                Ok(SyncMarketingConsentIntentResult::Missing)
            }
            ConsentWorkerClaimOutcome::Deferred { .. } => {
                tx.commit()
                    .await
                    .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                Ok(SyncMarketingConsentIntentResult::Deferred)
            }
            ConsentWorkerClaimOutcome::Terminal(status) => {
                if status == ConsentWorkerTerminalStatus::Blocked {
                    // BLOCKED can mean a grant was accepted before its lease was lost.
                    // Ask the coordinator to durably confirm or schedule any repair before ACK.
                    self.repair_raced_grant(&mut tx, intent_id).await?;
                }
                tx.commit()
                    .await
                    .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                Ok(terminal_result(status))
            }
        }
    }
}

impl<U, W, C, P> SyncMarketingConsentIntentHandler<U, W, C, P>
where
    U: UnitOfWork,
    W: MarketingConsentIntentWorker<U::Tx>,
    C: MarketingConsentIntentsFactory<U::Tx>,
    P: MarketingEmailConsentWriter,
{
    async fn execute_claim(
        &self,
        claim: ConsentWorkerClaim,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        validate_intent(&claim.intent)?;

        // This short transaction is the authoritative local fence. It is committed before
        // all provider reads and writes, so no database lock spans network I/O.
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
        let preflight = self
            .worker
            .recheck(&mut tx, &claim)
            .await
            .map_err(map_persistence_error)?;
        tx.commit()
            .await
            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;

        let ready = match preflight {
            ConsentWorkerRecheckOutcome::Ready(intent) => intent,
            ConsentWorkerRecheckOutcome::Terminal(status) => {
                if status == ConsentWorkerTerminalStatus::Blocked
                    && claim.prior_attempt_write_ambiguous
                {
                    // A recovered lease may represent an earlier accepted grant, even when
                    // this attempt discovers that the local fence has since closed.
                    self.provider
                        .current_state(&claim.intent.email)
                        .await
                        .map_err(map_provider_error)?;
                    let mut tx = self
                        .unit_of_work
                        .begin()
                        .await
                        .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                    self.repair_raced_grant(&mut tx, claim.intent.intent_id)
                        .await?;
                    tx.commit()
                        .await
                        .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                }
                return Ok(terminal_result(status));
            }
            ConsentWorkerRecheckOutcome::Missing | ConsentWorkerRecheckOutcome::LeaseLost => {
                return Ok(SyncMarketingConsentIntentResult::Deferred);
            }
        };

        // An expired IN_PROGRESS lease may have sent a Loops write before its worker died.
        // Read current provider state first; for grants, an absent or non-subscribed state is
        // conservatively terminal and is never repaired by another subscription write.
        if claim.prior_attempt_write_ambiguous {
            let observed = self
                .provider
                .current_state(&ready.email)
                .await
                .map_err(map_provider_error)?;
            if ready.desired {
                return self
                    .finalize_provider_result(
                        &claim,
                        if state_confirms_grant(&observed) {
                            applied_receipt(&observed)
                        } else {
                            Receipt::Blocked("AMBIGUOUS_GRANT_UNCONFIRMED")
                        },
                    )
                    .await;
            }
            if state_confirms_revoke(&observed) {
                return self
                    .finalize_provider_result(&claim, applied_receipt(&observed))
                    .await;
            }
            // Revoke is idempotent against its immutable address and can safely be retried.
        }

        let provider_result = if ready.desired {
            self.provider.grant(&ready).await
        } else {
            self.provider.revoke(&ready).await
        };

        match provider_result {
            Ok(MarketingEmailConsentOutcome::Applied { contact_id }) => {
                self.finalize_provider_result(&claim, Receipt::Applied(Some(contact_id)))
                    .await
            }
            Ok(MarketingEmailConsentOutcome::AlreadyApplied { contact_id }) => {
                self.finalize_provider_result(&claim, Receipt::Applied(contact_id))
                    .await
            }
            Ok(MarketingEmailConsentOutcome::BlockedByProviderPreferences) => {
                self.finalize_provider_result(
                    &claim,
                    Receipt::Blocked("PROVIDER_PREFERENCES_BLOCKED"),
                )
                .await
            }
            Err(MarketingEmailConsentError::AcceptanceUnknown)
            | Err(MarketingEmailConsentError::Protocol { .. }) => {
                self.reconcile_ambiguous_write(&claim, &ready).await
            }
            Err(error) => match definite_no_write_reason(error) {
                Some(reason_code) => self.release_for_retry(&claim, reason_code).await,
                None if error == MarketingEmailConsentError::IneligibleIntent => {
                    Err(SyncMarketingConsentIntentError::InvalidPersistedState)
                }
                None => Err(SyncMarketingConsentIntentError::Persistence),
            },
        }
    }

    async fn reconcile_ambiguous_write(
        &self,
        claim: &ConsentWorkerClaim,
        intent: &ConsentIntent,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        let observed = self
            .provider
            .current_state(&intent.email)
            .await
            .map_err(map_provider_error)?;
        if intent.desired {
            if state_confirms_grant(&observed) {
                self.finalize_provider_result(claim, applied_receipt(&observed))
                    .await
            } else {
                self.finalize_provider_result(
                    claim,
                    Receipt::Blocked("AMBIGUOUS_GRANT_UNCONFIRMED"),
                )
                .await
            }
        } else if state_confirms_revoke(&observed) {
            self.finalize_provider_result(claim, applied_receipt(&observed))
                .await
        } else {
            // A revoke remains safe to retry; never convert an unresolved read into success.
            Err(SyncMarketingConsentIntentError::Persistence)
        }
    }

    async fn finalize_provider_result(
        &self,
        claim: &ConsentWorkerClaim,
        receipt: Receipt,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        let completed_at = OffsetDateTime::now_utc();
        let mut replay_only = false;
        // A commit response can be lost after PostgreSQL committed. Retry this exact result
        // tuple and timestamp only; never repeat the Loops operation while saving its receipt.
        for _ in 0..2 {
            let mut tx = self
                .unit_of_work
                .begin()
                .await
                .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
            if replay_only {
                let confirmed = self
                    .finalize(&mut tx, claim, receipt.as_finalization(), completed_at)
                    .await;
                match confirmed {
                    Ok(true) => {
                        tx.commit()
                            .await
                            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                        return Ok(receipt.result());
                    }
                    Ok(false) => {
                        self.repair_if_grant_lost_fence(&mut tx, claim).await?;
                        tx.commit()
                            .await
                            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                        return Ok(SyncMarketingConsentIntentResult::Deferred);
                    }
                    Err(_) => continue,
                }
            }

            let recheck = match self.worker.recheck(&mut tx, claim).await {
                Ok(recheck) => recheck,
                Err(_) => {
                    replay_only = true;
                    continue;
                }
            };
            match recheck {
                ConsentWorkerRecheckOutcome::Ready(_) => {
                    let finalized = match self
                        .finalize(&mut tx, claim, receipt.as_finalization(), completed_at)
                        .await
                    {
                        Ok(finalized) => finalized,
                        Err(_) => {
                            replay_only = true;
                            continue;
                        }
                    };
                    if !finalized {
                        tx.commit()
                            .await
                            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                        return Ok(SyncMarketingConsentIntentResult::Deferred);
                    }
                    match tx.commit().await {
                        Ok(()) => return Ok(receipt.result()),
                        Err(_) => replay_only = true,
                    }
                }
                ConsentWorkerRecheckOutcome::Terminal(status) => {
                    self.repair_if_grant_lost_fence(&mut tx, claim).await?;
                    tx.commit()
                        .await
                        .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                    return Ok(terminal_result(status));
                }
                ConsentWorkerRecheckOutcome::Missing | ConsentWorkerRecheckOutcome::LeaseLost => {
                    tx.commit()
                        .await
                        .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
                    return Ok(SyncMarketingConsentIntentResult::Deferred);
                }
            }
        }
        Err(SyncMarketingConsentIntentError::Transaction)
    }

    async fn release_for_retry(
        &self,
        claim: &ConsentWorkerClaim,
        reason_code: &str,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
        let released = self
            .worker
            .release_for_retry(&mut tx, claim, reason_code)
            .await
            .map_err(map_persistence_error)?;
        tx.commit()
            .await
            .map_err(|_| SyncMarketingConsentIntentError::Transaction)?;
        Ok(if released {
            SyncMarketingConsentIntentResult::Retryable
        } else {
            SyncMarketingConsentIntentResult::Deferred
        })
    }

    async fn finalize(
        &self,
        tx: &mut U::Tx,
        claim: &ConsentWorkerClaim,
        result: ConsentWorkerFinalization<'_>,
        completed_at: OffsetDateTime,
    ) -> Result<bool, MarketingConsentIntentError> {
        self.worker
            .finalize_claim(tx, claim, result, completed_at)
            .await
    }

    async fn repair_if_grant_lost_fence(
        &self,
        tx: &mut U::Tx,
        claim: &ConsentWorkerClaim,
    ) -> Result<(), SyncMarketingConsentIntentError> {
        if claim.intent.desired {
            self.repair_raced_grant(tx, claim.intent.intent_id).await?;
        }
        Ok(())
    }

    async fn repair_raced_grant(
        &self,
        tx: &mut U::Tx,
        intent_id: MarketingConsentSyncIntentId,
    ) -> Result<GrantRaceRepairOutcome, SyncMarketingConsentIntentError> {
        let changed_at = OffsetDateTime::now_utc();
        MarketingConsentCoordinator::new(tx, &self.intents)
            .repair_raced_grant_if_needed(intent_id, changed_at)
            .await
            .map_err(map_coordinator_error)
    }
}

#[derive(Clone)]
enum Receipt {
    Applied(Option<String>),
    Blocked(&'static str),
}

impl Receipt {
    fn as_finalization(&self) -> ConsentWorkerFinalization<'_> {
        match self {
            Self::Applied(contact_id) => ConsentWorkerFinalization::Applied {
                provider_contact_id: contact_id.as_deref(),
            },
            Self::Blocked(error_code) => ConsentWorkerFinalization::Blocked { error_code },
        }
    }

    const fn result(&self) -> SyncMarketingConsentIntentResult {
        match self {
            Self::Applied(_) => SyncMarketingConsentIntentResult::Applied,
            Self::Blocked(_) => SyncMarketingConsentIntentResult::Blocked,
        }
    }
}

fn applied_receipt(state: &MarketingEmailSubscriptionState) -> Receipt {
    match state {
        MarketingEmailSubscriptionState::Missing => Receipt::Applied(None),
        MarketingEmailSubscriptionState::Present { contact_id, .. } => {
            Receipt::Applied(Some(contact_id.clone()))
        }
    }
}

fn state_confirms_grant(state: &MarketingEmailSubscriptionState) -> bool {
    matches!(
        state,
        MarketingEmailSubscriptionState::Present {
            globally_subscribed: true,
            on_target_list: true,
            suppressed: false,
            ..
        }
    )
}

fn state_confirms_revoke(state: &MarketingEmailSubscriptionState) -> bool {
    matches!(state, MarketingEmailSubscriptionState::Missing)
        || matches!(
            state,
            MarketingEmailSubscriptionState::Present {
                globally_subscribed: false,
                on_target_list: false,
                ..
            }
        )
}

fn validate_intent(intent: &ConsentIntent) -> Result<(), SyncMarketingConsentIntentError> {
    let valid = matches!(
        (intent.subject, intent.desired, intent.source),
        (
            ConsentSubject::User(_),
            true,
            ConsentIntentSource::CognitoSignup | ConsentIntentSource::AuraDoubleOptIn,
        ) | (
            ConsentSubject::EmailOnly,
            true,
            ConsentIntentSource::AuraDoubleOptIn
        ) | (
            ConsentSubject::User(_),
            false,
            ConsentIntentSource::UserWithdrawal
                | ConsentIntentSource::UserDeletion
                | ConsentIntentSource::ProviderRaceRepair,
        ) | (
            ConsentSubject::EmailOnly,
            false,
            ConsentIntentSource::EmailOnlyWithdrawal | ConsentIntentSource::ProviderRaceRepair,
        )
    );
    if valid {
        Ok(())
    } else {
        Err(SyncMarketingConsentIntentError::InvalidPersistedState)
    }
}

fn terminal_result(status: ConsentWorkerTerminalStatus) -> SyncMarketingConsentIntentResult {
    match status {
        ConsentWorkerTerminalStatus::Applied => SyncMarketingConsentIntentResult::Applied,
        ConsentWorkerTerminalStatus::Superseded => SyncMarketingConsentIntentResult::Superseded,
        ConsentWorkerTerminalStatus::Blocked => SyncMarketingConsentIntentResult::Blocked,
        ConsentWorkerTerminalStatus::Failed => SyncMarketingConsentIntentResult::AlreadyTerminal,
    }
}

fn map_persistence_error(error: MarketingConsentIntentError) -> SyncMarketingConsentIntentError {
    match error {
        MarketingConsentIntentError::InvalidPersistedState
        | MarketingConsentIntentError::InvalidInput => {
            SyncMarketingConsentIntentError::InvalidPersistedState
        }
        _ => SyncMarketingConsentIntentError::Persistence,
    }
}

fn map_coordinator_error(
    error: CoordinateMarketingConsentError,
) -> SyncMarketingConsentIntentError {
    match error {
        CoordinateMarketingConsentError::InvalidPersistedState => {
            SyncMarketingConsentIntentError::InvalidPersistedState
        }
        _ => SyncMarketingConsentIntentError::Persistence,
    }
}

fn map_provider_error(_: MarketingEmailConsentError) -> SyncMarketingConsentIntentError {
    SyncMarketingConsentIntentError::Persistence
}

fn definite_no_write_reason(error: MarketingEmailConsentError) -> Option<&'static str> {
    match error {
        MarketingEmailConsentError::NotSent => Some("NOT_SENT"),
        MarketingEmailConsentError::Rejected { .. } => Some("PROVIDER_REJECTED"),
        MarketingEmailConsentError::Throttled { .. } => Some("THROTTLED"),
        MarketingEmailConsentError::ReadUnavailable => Some("PREWRITE_READ_UNAVAILABLE"),
        MarketingEmailConsentError::InvalidEmail => Some("INVALID_EMAIL"),
        MarketingEmailConsentError::AcceptanceUnknown
        | MarketingEmailConsentError::Protocol { .. }
        | MarketingEmailConsentError::IneligibleIntent => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ConsentUser, MarketingConsentIntents, MarketingConsentIntentsFactory, NewsletterProfile,
    };
    use application::transaction::TransactionError;
    use serde_email::Email;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use user_core::user_id::UserId;

    struct TestTx;

    #[async_trait::async_trait]
    impl Transaction for TestTx {
        async fn commit(self) -> Result<(), TransactionError> {
            Ok(())
        }
    }

    struct TestUnitOfWork;

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTx)
        }
    }

    struct TestWorker {
        claims: Mutex<Vec<ConsentWorkerClaimOutcome>>,
        rechecks: Mutex<Vec<ConsentWorkerRecheckOutcome>>,
        finalizations: Mutex<Vec<&'static str>>,
        releases: Mutex<Vec<String>>,
    }

    impl TestWorker {
        fn new(
            claims: impl IntoIterator<Item = ConsentWorkerClaimOutcome>,
            rechecks: impl IntoIterator<Item = ConsentWorkerRecheckOutcome>,
        ) -> Self {
            Self {
                claims: Mutex::new(claims.into_iter().collect()),
                rechecks: Mutex::new(rechecks.into_iter().collect()),
                finalizations: Mutex::new(Vec::new()),
                releases: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl MarketingConsentIntentWorker<TestTx> for TestWorker {
        async fn claim_by_id(
            &self,
            _: &mut TestTx,
            _: MarketingConsentSyncIntentId,
        ) -> Result<ConsentWorkerClaimOutcome, MarketingConsentIntentError> {
            Ok(self.claims.lock().unwrap().remove(0))
        }

        async fn recheck(
            &self,
            _: &mut TestTx,
            _: &ConsentWorkerClaim,
        ) -> Result<ConsentWorkerRecheckOutcome, MarketingConsentIntentError> {
            Ok(self.rechecks.lock().unwrap().remove(0))
        }

        async fn finalize_claim(
            &self,
            _: &mut TestTx,
            _: &ConsentWorkerClaim,
            result: ConsentWorkerFinalization<'_>,
            _: OffsetDateTime,
        ) -> Result<bool, MarketingConsentIntentError> {
            let label = match result {
                ConsentWorkerFinalization::Applied { .. } => "APPLIED",
                ConsentWorkerFinalization::Failed { .. } => "FAILED",
                ConsentWorkerFinalization::Blocked { .. } => "BLOCKED",
            };
            self.finalizations.lock().unwrap().push(label);
            Ok(true)
        }

        async fn release_for_retry(
            &self,
            _: &mut TestTx,
            _: &ConsentWorkerClaim,
            reason_code: &str,
        ) -> Result<bool, MarketingConsentIntentError> {
            self.releases.lock().unwrap().push(reason_code.to_owned());
            Ok(true)
        }
    }

    struct TestIntentsFactory {
        repairs: Arc<AtomicUsize>,
    }

    impl MarketingConsentIntentsFactory<TestTx> for TestIntentsFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TestTx,
        ) -> impl MarketingConsentIntents + 'tx {
            TestIntents {
                repairs: Arc::clone(&self.repairs),
            }
        }
    }

    struct TestIntents {
        repairs: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl MarketingConsentIntents for TestIntents {
        async fn lock_recipient(&mut self, _: &Email) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn lock_source_key(&mut self, _: &str) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_by_source_key(
            &mut self,
            _: &str,
        ) -> Result<Option<ConsentIntent>, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_user_by_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn find_user_by_email(
            &mut self,
            _: &Email,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn record_user_transition(
            &mut self,
            _: &ConsentUser,
            _: bool,
            _: ConsentIntentSource,
            _: &str,
            _: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn record_email_only_intent(
            &mut self,
            _: &Email,
            _: bool,
            _: ConsentIntentSource,
            _: &str,
            _: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
        async fn apply_provider_withdrawal(
            &mut self,
            _: &ConsentUser,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn cancel_provider_backsync(
            &mut self,
            _: &Email,
        ) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn invalidate_newsletter_confirmation_challenges(
            &mut self,
            _: &Email,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }
        async fn repair_raced_grant_if_needed(
            &mut self,
            _: MarketingConsentSyncIntentId,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<GrantRaceRepairOutcome, MarketingConsentIntentError> {
            self.repairs.fetch_add(1, Ordering::SeqCst);
            Ok(GrantRaceRepairOutcome::NoRepairNeeded)
        }
        async fn record_user_deletion(
            &mut self,
            _: &ConsentUser,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            unreachable!()
        }
    }

    struct TestProvider {
        states: Mutex<Vec<MarketingEmailSubscriptionState>>,
        grant: Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>,
        grants: AtomicUsize,
        revokes: AtomicUsize,
    }

    impl TestProvider {
        fn new(
            states: impl IntoIterator<Item = MarketingEmailSubscriptionState>,
            grant: Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>,
        ) -> Self {
            Self {
                states: Mutex::new(states.into_iter().collect()),
                grant,
                grants: AtomicUsize::new(0),
                revokes: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl MarketingEmailConsentWriter for TestProvider {
        async fn current_state(
            &self,
            _: &Email,
        ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError> {
            let mut states = self.states.lock().unwrap();
            Ok(states.remove(0))
        }

        async fn grant(
            &self,
            _: &ConsentIntent,
        ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
            self.grants.fetch_add(1, Ordering::SeqCst);
            self.grant.clone()
        }

        async fn revoke(
            &self,
            _: &ConsentIntent,
        ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
            self.revokes.fetch_add(1, Ordering::SeqCst);
            Ok(MarketingEmailConsentOutcome::AlreadyApplied { contact_id: None })
        }
    }

    #[tokio::test]
    async fn abandoned_grant_reconciles_and_never_resubscribes_when_provider_state_is_unclear() {
        let claim = test_claim(2);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        let provider = TestProvider::new(
            [MarketingEmailSubscriptionState::Missing],
            Ok(MarketingEmailConsentOutcome::Applied {
                contact_id: "unexpected".into(),
            }),
        );
        let use_case = handler(worker, provider);

        let result = use_case.execute(claim.intent.intent_id).await;

        assert_eq!(Ok(SyncMarketingConsentIntentResult::Blocked), result);
        assert_eq!(0, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(
            vec!["BLOCKED"],
            *use_case.worker.finalizations.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn grant_lost_during_provider_call_is_repaired_before_the_job_completes() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Terminal(ConsentWorkerTerminalStatus::Blocked),
            ],
        );
        let provider = TestProvider::new(
            [],
            Ok(MarketingEmailConsentOutcome::Applied {
                contact_id: "contact-123".into(),
            }),
        );
        let repairs = Arc::new(AtomicUsize::new(0));
        let use_case = SyncMarketingConsentIntentHandler::new(
            TestUnitOfWork,
            worker,
            TestIntentsFactory {
                repairs: Arc::clone(&repairs),
            },
            provider,
        );

        let result = use_case.execute(claim.intent.intent_id).await;

        assert_eq!(Ok(SyncMarketingConsentIntentResult::Blocked), result);
        assert_eq!(1, repairs.load(Ordering::SeqCst));
        assert!(use_case.worker.finalizations.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn terminal_blocked_claim_confirms_repair_before_it_can_be_acknowledged() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Terminal(
                ConsentWorkerTerminalStatus::Blocked,
            )],
            [],
        );
        let provider = TestProvider::new(
            [],
            Ok(MarketingEmailConsentOutcome::AlreadyApplied { contact_id: None }),
        );
        let repairs = Arc::new(AtomicUsize::new(0));
        let use_case = SyncMarketingConsentIntentHandler::new(
            TestUnitOfWork,
            worker,
            TestIntentsFactory {
                repairs: Arc::clone(&repairs),
            },
            provider,
        );

        let result = use_case.execute(claim.intent.intent_id).await;

        assert_eq!(Ok(SyncMarketingConsentIntentResult::Blocked), result);
        assert_eq!(1, repairs.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn definite_provider_rejection_returns_intent_to_retryable_pending_state() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [ConsentWorkerRecheckOutcome::Ready(claim.intent.clone())],
        );
        let provider = TestProvider::new(
            [],
            Err(MarketingEmailConsentError::Throttled { status: Some(429) }),
        );
        let use_case = handler(worker, provider);

        assert_eq!(
            Ok(SyncMarketingConsentIntentResult::Retryable),
            use_case.execute(claim.intent.intent_id).await
        );
        assert_eq!(vec!["THROTTLED"], *use_case.worker.releases.lock().unwrap());
    }

    fn handler(
        worker: TestWorker,
        provider: TestProvider,
    ) -> SyncMarketingConsentIntentHandler<
        TestUnitOfWork,
        TestWorker,
        TestIntentsFactory,
        TestProvider,
    > {
        SyncMarketingConsentIntentHandler::new(
            TestUnitOfWork,
            worker,
            TestIntentsFactory {
                repairs: Arc::new(AtomicUsize::new(0)),
            },
            provider,
        )
    }

    fn test_claim(attempt_count: u32) -> ConsentWorkerClaim {
        let now = OffsetDateTime::now_utc();
        ConsentWorkerClaim {
            intent: ConsentIntent {
                intent_id: MarketingConsentSyncIntentId::new(),
                source_key: "synthetic-proof-key".into(),
                subject: ConsentSubject::EmailOnly,
                source: ConsentIntentSource::AuraDoubleOptIn,
                email: Email::try_from("consent-worker@example.test").unwrap(),
                profile_snapshot: None,
                desired: true,
            },
            recipient_key: "a".repeat(64),
            consent_revision: None,
            not_after: Some(now + time::Duration::days(1)),
            changed_at: now,
            lease_token: "synthetic-lease-token".into(),
            lease_expires_at: now + time::Duration::minutes(5),
            attempt_count,
            prior_attempt_write_ambiguous: attempt_count > 1,
        }
    }
}
