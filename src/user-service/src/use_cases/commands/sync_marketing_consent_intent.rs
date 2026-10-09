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

#[derive(Debug, thiserror::Error)]
pub enum SyncMarketingConsentIntentError {
    #[error("consent intent persistence unavailable")]
    Persistence,
    #[error("consent intent transaction outcome is unconfirmed")]
    Transaction(#[source] application::error::BoxError),
    #[error("consent intent reconciliation retry limit reached")]
    ReconciliationExhausted {
        #[source]
        source: Option<application::error::BoxError>,
    },
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
            .map_err(|source| SyncMarketingConsentIntentError::Transaction(Box::new(source)))?;
        let claim = self
            .worker
            .claim_by_id(&mut tx, intent_id)
            .await
            .map_err(map_persistence_error)?;
        match claim {
            ConsentWorkerClaimOutcome::Claimed(claim) => {
                tx.commit().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
                self.execute_claim(claim).await
            }
            ConsentWorkerClaimOutcome::Missing => {
                tx.commit().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
                Ok(SyncMarketingConsentIntentResult::Missing)
            }
            ConsentWorkerClaimOutcome::Deferred { .. } => {
                tx.commit().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
                Ok(SyncMarketingConsentIntentResult::Deferred)
            }
            ConsentWorkerClaimOutcome::Terminal(status) => {
                if status == ConsentWorkerTerminalStatus::Blocked {
                    // BLOCKED can mean a grant was accepted before its lease was lost.
                    // Ask the coordinator to durably confirm or schedule any repair before ACK.
                    self.repair_raced_grant(&mut tx, intent_id).await?;
                }
                tx.commit().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
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
            .map_err(|source| SyncMarketingConsentIntentError::Transaction(Box::new(source)))?;
        let preflight = self
            .worker
            .recheck(&mut tx, &claim)
            .await
            .map_err(map_persistence_error)?;
        tx.commit()
            .await
            .map_err(|source| SyncMarketingConsentIntentError::Transaction(Box::new(source)))?;

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
                    let mut tx = self.unit_of_work.begin().await.map_err(|source| {
                        SyncMarketingConsentIntentError::Transaction(Box::new(source))
                    })?;
                    self.repair_raced_grant(&mut tx, claim.intent.intent_id)
                        .await?;
                    tx.commit().await.map_err(|source| {
                        SyncMarketingConsentIntentError::Transaction(Box::new(source))
                    })?;
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
        let mut last_error = None;
        // A commit response can be lost after PostgreSQL committed. Retry this exact result
        // tuple and timestamp only; never repeat the Loops operation while saving its receipt.
        for _ in 0..2 {
            let mut tx =
                self.unit_of_work.begin().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
            if replay_only {
                let confirmed = self
                    .finalize(&mut tx, claim, receipt.as_finalization(), completed_at)
                    .await;
                match confirmed {
                    Ok(true) => {
                        tx.commit().await.map_err(|source| {
                            SyncMarketingConsentIntentError::Transaction(Box::new(source))
                        })?;
                        return Ok(receipt.result());
                    }
                    Ok(false) => {
                        self.repair_if_grant_lost_fence(&mut tx, claim).await?;
                        tx.commit().await.map_err(|source| {
                            SyncMarketingConsentIntentError::Transaction(Box::new(source))
                        })?;
                        return Ok(SyncMarketingConsentIntentResult::Deferred);
                    }
                    Err(source) => {
                        last_error = Some(application::error::box_error(source));
                        continue;
                    }
                }
            }

            let recheck = match self.worker.recheck(&mut tx, claim).await {
                Ok(recheck) => recheck,
                Err(source) => {
                    last_error = Some(application::error::box_error(source));
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
                        Err(source) => {
                            last_error = Some(application::error::box_error(source));
                            replay_only = true;
                            continue;
                        }
                    };
                    if !finalized {
                        self.repair_if_grant_lost_fence(&mut tx, claim).await?;
                        tx.commit().await.map_err(|source| {
                            SyncMarketingConsentIntentError::Transaction(Box::new(source))
                        })?;
                        return Ok(SyncMarketingConsentIntentResult::Deferred);
                    }
                    match tx.commit().await {
                        Ok(()) => return Ok(receipt.result()),
                        Err(source) => {
                            last_error = Some(application::error::box_error(source));
                            replay_only = true;
                        }
                    }
                }
                ConsentWorkerRecheckOutcome::Terminal(status) => {
                    self.repair_if_grant_lost_fence(&mut tx, claim).await?;
                    tx.commit().await.map_err(|source| {
                        SyncMarketingConsentIntentError::Transaction(Box::new(source))
                    })?;
                    return Ok(terminal_result(status));
                }
                ConsentWorkerRecheckOutcome::Missing | ConsentWorkerRecheckOutcome::LeaseLost => {
                    tx.commit().await.map_err(|source| {
                        SyncMarketingConsentIntentError::Transaction(Box::new(source))
                    })?;
                    return Ok(SyncMarketingConsentIntentResult::Deferred);
                }
            }
        }
        Err(SyncMarketingConsentIntentError::ReconciliationExhausted { source: last_error })
    }

    async fn release_for_retry(
        &self,
        claim: &ConsentWorkerClaim,
        reason_code: &str,
    ) -> Result<SyncMarketingConsentIntentResult, SyncMarketingConsentIntentError> {
        let mut last_error = None;
        // A commit error has an unknown outcome. Retry this exact lease/reason in a
        // fresh transaction: persistence confirms an already-committed marker or
        // reapplies the release if the first transaction rolled back.
        for _ in 0..2 {
            let mut tx =
                self.unit_of_work.begin().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
            let released = self
                .worker
                .release_for_retry(&mut tx, claim, reason_code)
                .await
                .map_err(map_persistence_error)?;
            if !released {
                tx.commit().await.map_err(|source| {
                    SyncMarketingConsentIntentError::Transaction(Box::new(source))
                })?;
                return Ok(SyncMarketingConsentIntentResult::Deferred);
            }
            match tx.commit().await {
                Ok(()) => return Ok(SyncMarketingConsentIntentResult::Retryable),
                Err(source) => last_error = Some(application::error::box_error(source)),
            }
        }
        Err(SyncMarketingConsentIntentError::ReconciliationExhausted { source: last_error })
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
        MarketingEmailConsentError::PreWriteProtocol { .. } => Some("PREWRITE_PROTOCOL"),
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

    struct TestTx {
        commits: Arc<AtomicUsize>,
        unconfirmed_commits: Vec<usize>,
    }

    #[async_trait::async_trait]
    impl Transaction for TestTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let commit_number = self.commits.fetch_add(1, Ordering::SeqCst) + 1;
            if self.unconfirmed_commits.contains(&commit_number) {
                // Model a commit that reached PostgreSQL but whose response was lost.
                Err(TransactionError::CommitFailed(
                    application::error::static_error("test transaction failure"),
                ))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct TestUnitOfWork {
        commits: Arc<AtomicUsize>,
        unconfirmed_commits: Vec<usize>,
    }

    impl TestUnitOfWork {
        fn with_unconfirmed_commit(commit_number: usize) -> Self {
            Self {
                commits: Arc::new(AtomicUsize::new(0)),
                unconfirmed_commits: vec![commit_number],
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for TestUnitOfWork {
        type Tx = TestTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(TestTx {
                commits: Arc::clone(&self.commits),
                unconfirmed_commits: self.unconfirmed_commits.clone(),
            })
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RecordedFinalization {
        status: &'static str,
        provider_contact_id: Option<String>,
        error_code: Option<String>,
        completed_at: OffsetDateTime,
    }

    struct TestWorker {
        claims: Mutex<Vec<ConsentWorkerClaimOutcome>>,
        rechecks: Mutex<Vec<ConsentWorkerRecheckOutcome>>,
        finalizations: Mutex<Vec<&'static str>>,
        finalization_calls: Mutex<Vec<RecordedFinalization>>,
        finalization_failures: Mutex<usize>,
        finalization_results: Mutex<Vec<bool>>,
        releases: Mutex<Vec<String>>,
        release_calls: Mutex<Vec<RecordedRelease>>,
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
                finalization_calls: Mutex::new(Vec::new()),
                finalization_failures: Mutex::new(0),
                finalization_results: Mutex::new(Vec::new()),
                releases: Mutex::new(Vec::new()),
                release_calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RecordedRelease {
        intent_id: MarketingConsentSyncIntentId,
        lease_token: String,
        attempt_count: u32,
        reason_code: String,
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
            completed_at: OffsetDateTime,
        ) -> Result<bool, MarketingConsentIntentError> {
            let (label, provider_contact_id, error_code) = match result {
                ConsentWorkerFinalization::Applied {
                    provider_contact_id,
                } => ("APPLIED", provider_contact_id.map(str::to_owned), None),
                ConsentWorkerFinalization::Failed { error_code } => {
                    ("FAILED", None, Some(error_code.to_owned()))
                }
                ConsentWorkerFinalization::Blocked { error_code } => {
                    ("BLOCKED", None, Some(error_code.to_owned()))
                }
            };
            self.finalizations.lock().unwrap().push(label);
            self.finalization_calls
                .lock()
                .unwrap()
                .push(RecordedFinalization {
                    status: label,
                    provider_contact_id,
                    error_code,
                    completed_at,
                });
            let mut failures = self.finalization_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(MarketingConsentIntentError::TemporarilyUnavailable {
                    source: application::error::static_error("scripted finalization failure"),
                });
            }
            let mut results = self.finalization_results.lock().unwrap();
            Ok(if results.is_empty() {
                true
            } else {
                results.remove(0)
            })
        }

        async fn release_for_retry(
            &self,
            _: &mut TestTx,
            claim: &ConsentWorkerClaim,
            reason_code: &str,
        ) -> Result<bool, MarketingConsentIntentError> {
            self.releases.lock().unwrap().push(reason_code.to_owned());
            self.release_calls.lock().unwrap().push(RecordedRelease {
                intent_id: claim.intent.intent_id,
                lease_token: claim.lease_token.clone(),
                attempt_count: claim.attempt_count,
                reason_code: reason_code.to_owned(),
            });
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
        grant_results: Mutex<Vec<Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>>>,
        grants: AtomicUsize,
        grant_writes: AtomicUsize,
        revokes: AtomicUsize,
        state_reads: AtomicUsize,
    }

    impl TestProvider {
        fn new(
            states: impl IntoIterator<Item = MarketingEmailSubscriptionState>,
            grant: Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>,
        ) -> Self {
            Self {
                states: Mutex::new(states.into_iter().collect()),
                grant_results: Mutex::new(vec![grant]),
                grants: AtomicUsize::new(0),
                grant_writes: AtomicUsize::new(0),
                revokes: AtomicUsize::new(0),
                state_reads: AtomicUsize::new(0),
            }
        }

        fn with_grant_results(
            states: impl IntoIterator<Item = MarketingEmailSubscriptionState>,
            grant_results: impl IntoIterator<
                Item = Result<MarketingEmailConsentOutcome, MarketingEmailConsentError>,
            >,
        ) -> Self {
            Self {
                states: Mutex::new(states.into_iter().collect()),
                grant_results: Mutex::new(grant_results.into_iter().collect()),
                grants: AtomicUsize::new(0),
                grant_writes: AtomicUsize::new(0),
                revokes: AtomicUsize::new(0),
                state_reads: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl MarketingEmailConsentWriter for TestProvider {
        async fn current_state(
            &self,
            _: &Email,
        ) -> Result<MarketingEmailSubscriptionState, MarketingEmailConsentError> {
            self.state_reads.fetch_add(1, Ordering::SeqCst);
            let mut states = self.states.lock().unwrap();
            Ok(states.remove(0))
        }

        async fn grant(
            &self,
            _: &ConsentIntent,
        ) -> Result<MarketingEmailConsentOutcome, MarketingEmailConsentError> {
            self.grants.fetch_add(1, Ordering::SeqCst);
            let result = self.grant_results.lock().unwrap().remove(0);
            if matches!(
                result,
                Ok(_)
                    | Err(MarketingEmailConsentError::AcceptanceUnknown)
                    | Err(MarketingEmailConsentError::Protocol { .. })
            ) {
                self.grant_writes.fetch_add(1, Ordering::SeqCst);
            }
            result
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

        assert!(matches!(
            result,
            Ok(SyncMarketingConsentIntentResult::Blocked)
        ));
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
            TestUnitOfWork::default(),
            worker,
            TestIntentsFactory {
                repairs: Arc::clone(&repairs),
            },
            provider,
        );

        let result = use_case.execute(claim.intent.intent_id).await;

        assert!(matches!(
            result,
            Ok(SyncMarketingConsentIntentResult::Blocked)
        ));
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
            TestUnitOfWork::default(),
            worker,
            TestIntentsFactory {
                repairs: Arc::clone(&repairs),
            },
            provider,
        );

        let result = use_case.execute(claim.intent.intent_id).await;

        assert!(matches!(
            result,
            Ok(SyncMarketingConsentIntentResult::Blocked)
        ));
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

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Retryable)
        ));
        assert_eq!(vec!["THROTTLED"], *use_case.worker.releases.lock().unwrap());
    }

    #[tokio::test]
    async fn unconfirmed_no_write_release_replays_same_marker_without_ambiguous_reconciliation() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [ConsentWorkerRecheckOutcome::Ready(claim.intent.clone())],
        );
        let provider = TestProvider::new(
            [],
            Err(MarketingEmailConsentError::Throttled { status: Some(429) }),
        );
        // Claim and preflight commits are 1 and 2; commit 3 is the release whose
        // response is lost after PostgreSQL committed. Commit 4 confirms its replay.
        let uow = TestUnitOfWork::with_unconfirmed_commit(3);
        let commits = Arc::clone(&uow.commits);
        let use_case = SyncMarketingConsentIntentHandler::new(
            uow,
            worker,
            TestIntentsFactory {
                repairs: Arc::new(AtomicUsize::new(0)),
            },
            provider,
        );

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Retryable)
        ));
        assert_eq!(4, commits.load(Ordering::SeqCst));
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(0, use_case.provider.grant_writes.load(Ordering::SeqCst));
        assert_eq!(0, use_case.provider.state_reads.load(Ordering::SeqCst));

        let release_calls = use_case.worker.release_calls.lock().unwrap();
        assert_eq!(2, release_calls.len());
        assert_eq!(release_calls[0], release_calls[1]);
        assert_eq!(claim.lease_token, release_calls[0].lease_token);
        assert_eq!(claim.attempt_count, release_calls[0].attempt_count);
        assert_eq!("THROTTLED", release_calls[0].reason_code);
    }

    #[tokio::test]
    async fn exhausted_release_retries_preserve_the_last_commit_cause() {
        use std::error::Error;

        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [ConsentWorkerRecheckOutcome::Ready(claim.intent.clone())],
        );
        let provider = TestProvider::new(
            [],
            Err(MarketingEmailConsentError::Throttled { status: Some(429) }),
        );
        let use_case = SyncMarketingConsentIntentHandler::new(
            TestUnitOfWork {
                unconfirmed_commits: vec![3, 4],
                ..Default::default()
            },
            worker,
            TestIntentsFactory {
                repairs: Arc::new(AtomicUsize::new(0)),
            },
            provider,
        );

        let error = use_case.execute(claim.intent.intent_id).await.unwrap_err();
        assert!(matches!(
            &error,
            SyncMarketingConsentIntentError::ReconciliationExhausted { .. }
        ));
        let transaction = error.source().unwrap();
        assert!(transaction.is::<TransactionError>());
        assert_eq!(
            "test transaction failure",
            transaction.source().unwrap().to_string()
        );
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(2, use_case.worker.release_calls.lock().unwrap().len());
    }

    #[tokio::test]
    async fn prewrite_protocol_failure_releases_and_recovers_without_losing_the_grant() {
        let claim = test_claim(1);
        let mut retry_claim = claim.clone();
        retry_claim.attempt_count = 2;
        retry_claim.prior_attempt_write_ambiguous = false;
        let worker = TestWorker::new(
            [
                ConsentWorkerClaimOutcome::Claimed(claim.clone()),
                ConsentWorkerClaimOutcome::Claimed(retry_claim),
            ],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        let provider = TestProvider::with_grant_results(
            [],
            [
                Err(MarketingEmailConsentError::PreWriteProtocol { status: Some(401) }),
                Ok(MarketingEmailConsentOutcome::Applied {
                    contact_id: "contact-123".into(),
                }),
            ],
        );
        let use_case = handler(worker, provider);

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Retryable)
        ));
        assert_eq!(
            vec!["PREWRITE_PROTOCOL"],
            *use_case.worker.releases.lock().unwrap()
        );
        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Applied)
        ));
        assert_eq!(2, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(1, use_case.provider.grant_writes.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn postwrite_protocol_failure_reconciles_instead_of_releasing_as_no_write() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        let provider = TestProvider::new(
            [MarketingEmailSubscriptionState::Present {
                contact_id: "contact-123".into(),
                globally_subscribed: true,
                on_target_list: true,
                suppressed: false,
            }],
            Err(MarketingEmailConsentError::Protocol { status: Some(200) }),
        );
        let use_case = handler(worker, provider);

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Applied)
        ));
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        assert!(use_case.worker.releases.lock().unwrap().is_empty());
        assert_eq!(
            vec!["APPLIED"],
            *use_case.worker.finalizations.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn lost_finalize_commit_replays_the_same_receipt_without_another_provider_grant() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        let provider = TestProvider::new(
            [],
            Ok(MarketingEmailConsentOutcome::Applied {
                contact_id: "contact-123".into(),
            }),
        );
        let uow = TestUnitOfWork::with_unconfirmed_commit(3);
        let commits = Arc::clone(&uow.commits);
        let use_case = SyncMarketingConsentIntentHandler::new(
            uow,
            worker,
            TestIntentsFactory {
                repairs: Arc::new(AtomicUsize::new(0)),
            },
            provider,
        );

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Applied)
        ));
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(4, commits.load(Ordering::SeqCst));
        let calls = use_case.worker.finalization_calls.lock().unwrap();
        assert_eq!(2, calls.len());
        assert_eq!(calls[0], calls[1]);
        assert_eq!("APPLIED", calls[0].status);
        assert_eq!(Some("contact-123".to_owned()), calls[0].provider_contact_id);
    }

    #[tokio::test]
    async fn unconfirmed_finalize_replays_the_exact_result_without_another_provider_grant() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        *worker.finalization_failures.lock().unwrap() = 1;
        let provider = TestProvider::new(
            [],
            Ok(MarketingEmailConsentOutcome::Applied {
                contact_id: "contact-123".into(),
            }),
        );
        let use_case = handler(worker, provider);

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Applied)
        ));
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        let calls = use_case.worker.finalization_calls.lock().unwrap();
        assert_eq!(2, calls.len());
        assert_eq!(calls[0], calls[1]);
    }

    #[tokio::test]
    async fn lost_lease_finalize_false_defers_and_checks_for_a_raced_grant_repair() {
        let claim = test_claim(1);
        let worker = TestWorker::new(
            [ConsentWorkerClaimOutcome::Claimed(claim.clone())],
            [
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
                ConsentWorkerRecheckOutcome::Ready(claim.intent.clone()),
            ],
        );
        worker.finalization_results.lock().unwrap().push(false);
        let provider = TestProvider::new(
            [],
            Ok(MarketingEmailConsentOutcome::Applied {
                contact_id: "contact-123".into(),
            }),
        );
        let repairs = Arc::new(AtomicUsize::new(0));
        let use_case = SyncMarketingConsentIntentHandler::new(
            TestUnitOfWork::default(),
            worker,
            TestIntentsFactory {
                repairs: Arc::clone(&repairs),
            },
            provider,
        );

        assert!(matches!(
            use_case.execute(claim.intent.intent_id).await,
            Ok(SyncMarketingConsentIntentResult::Deferred)
        ));
        assert_eq!(1, use_case.provider.grants.load(Ordering::SeqCst));
        assert_eq!(1, repairs.load(Ordering::SeqCst));
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
            TestUnitOfWork::default(),
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
