use crate::ports::{
    CognitoIdentity, ConsentIntent, ConsentIntentSource, ConsentSubject,
    MarketingConsentIntentError, MarketingConsentIntents, MarketingConsentIntentsFactory,
    NewsletterProfile, UserCognitoIdentityRegistry, UserCognitoIdentityRegistryError,
    UserCognitoIdentityRegistryFactory,
};
use crate::use_cases::commands::marketing_consent_evidence::{
    ConsentEvidenceAction, ConsentEvidenceSource, MarketingConsentEvidence,
};
use application::operation_context::{OperationContext, Principal};
use application::transaction::{Transaction, UnitOfWork};
use serde_email::Email;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId;
use user_core::user_id::UserId;

pub use crate::ports::GrantRaceRepairOutcome;

/// Only a trusted PostConfirmation registration path may submit this identity.
/// The binding is rechecked against the target User inside the transaction.
pub enum MarketingConsentDecision {
    CognitoSignup {
        identity: CognitoIdentity,
        user_id: UserId,
        email: Email,
    },
    /// Trusted accepted proof identity, not a request ID or caller-supplied verified flag.
    /// The top-level handler rejects anonymous requests; verified anonymous DOI flows
    /// call the transaction-bound coordinator from their trusted service use case.
    AcceptedDoubleOptIn {
        confirmation_id: String,
        email: Email,
    },
    UserWithdrawal {
        user_id: UserId,
        email: Email,
        action_id: String,
    },
    EmailOnlyWithdrawal {
        email: Email,
        action_id: String,
    },

    /// Stable deletion identity derives solely from the User ID, not the actor or request.
    UserDeletion {
        user_id: UserId,
    },
    /// Inbound provider state. Cancels pending grants and never creates an outbound echo.
    ProviderWithdrawal {
        email: Email,
        /// Timestamp of the accepted provider decision, not local receipt time.
        accepted_at: OffsetDateTime,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinateMarketingConsentError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid consent decision identity")]
    InvalidIdentity,
    #[error("registered user or identity not found")]
    UserNotFound,
    #[error("confirmed email does not match account email")]
    EmailMismatch,
    #[error("source key was used for another decision")]
    SourceKeyConflict,
    #[error("consent changed concurrently")]
    ConcurrencyConflict,
    #[error("invalid persisted consent state")]
    InvalidPersistedState,
    #[error("consent persistence unavailable")]
    TemporarilyUnavailable,
    #[error("failed to begin consent transaction")]
    BeginTransactionFailed,
    #[error("failed to commit consent transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CoordinateMarketingConsentUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        decision: MarketingConsentDecision,
    ) -> Result<Option<MarketingConsentSyncIntentId>, CoordinateMarketingConsentError>;
}

pub(crate) struct MarketingConsentDecisionResult {
    pub(crate) intent_id: MarketingConsentSyncIntentId,
    pub(crate) evidence: Option<MarketingConsentEvidence>,
}

pub struct CoordinateMarketingConsentHandler<U, I, C> {
    unit_of_work: U,
    identities: I,
    intents: C,
}

impl<U, I, C> CoordinateMarketingConsentHandler<U, I, C> {
    pub fn new(unit_of_work: U, identities: I, intents: C) -> Self {
        Self {
            unit_of_work,
            identities,
            intents,
        }
    }
}

#[async_trait::async_trait]
impl<U, I, C> CoordinateMarketingConsentUseCase for CoordinateMarketingConsentHandler<U, I, C>
where
    U: UnitOfWork,
    I: UserCognitoIdentityRegistryFactory<U::Tx>,
    C: MarketingConsentIntentsFactory<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        decision: MarketingConsentDecision,
    ) -> Result<Option<MarketingConsentSyncIntentId>, CoordinateMarketingConsentError> {
        match (&context.principal, &decision) {
            (Principal::System, _) => {}
            (Principal::User(actor), MarketingConsentDecision::UserWithdrawal { user_id, .. })
                if actor == user_id => {}
            _ => return Err(CoordinateMarketingConsentError::Forbidden),
        }
        // Validate proof/action identity before opening a transaction.
        decision_source_key(&decision)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CoordinateMarketingConsentError::BeginTransactionFailed)?;
        let now = OffsetDateTime::now_utc();
        let result = {
            let mut coordinator = MarketingConsentCoordinator::new(&mut tx, &self.intents);
            match decision {
                MarketingConsentDecision::CognitoSignup {
                    identity,
                    user_id,
                    email,
                } => Some(
                    coordinator
                        .cognito_signup_with_evidence(
                            &self.identities,
                            identity,
                            user_id,
                            email,
                            now,
                        )
                        .await?,
                ),
                MarketingConsentDecision::AcceptedDoubleOptIn {
                    confirmation_id,
                    email,
                } => Some(
                    coordinator
                        .accepted_double_opt_in_with_evidence(confirmation_id, email, now)
                        .await?,
                ),
                MarketingConsentDecision::UserWithdrawal {
                    user_id,
                    email,
                    action_id,
                } => Some(
                    coordinator
                        .user_withdrawal_with_evidence(user_id, email, action_id, now)
                        .await?,
                ),
                MarketingConsentDecision::EmailOnlyWithdrawal { email, action_id } => Some(
                    coordinator
                        .email_only_withdrawal_with_evidence(email, action_id, now)
                        .await?,
                ),
                MarketingConsentDecision::UserDeletion { user_id } => Some(
                    coordinator
                        .user_deletion_with_evidence(user_id, now)
                        .await?,
                ),
                MarketingConsentDecision::ProviderWithdrawal { email, accepted_at } => {
                    coordinator.provider_withdrawal(email, accepted_at).await?;
                    None
                }
            }
        };
        tx.commit()
            .await
            .map_err(|_| CoordinateMarketingConsentError::CommitTransactionFailed)?;
        Ok(result.map(|result| {
            if let Some(evidence) = result.evidence.as_ref() {
                evidence.emit_after_commit(Some(context));
            }
            result.intent_id
        }))
    }
}

/// Service-owned consent operations within a caller's transaction. Never begins or commits.
/// Callers must authorize the operation; trusted DOI proof acceptance must precede
/// `accepted_double_opt_in`. Signup binding is always checked here before any grant.
/// Deletion callers must enforce account-removal guards in the same transaction.
pub struct MarketingConsentCoordinator<'a, Tx, C> {
    tx: &'a mut Tx,
    intents: &'a C,
}

impl<'a, Tx, C> MarketingConsentCoordinator<'a, Tx, C>
where
    Tx: Transaction,
    C: MarketingConsentIntentsFactory<Tx>,
{
    pub fn new(tx: &'a mut Tx, intents: &'a C) -> Self {
        Self { tx, intents }
    }

    /// Verify the exact registered Cognito binding inside this transaction before granting.
    pub async fn cognito_signup<I: UserCognitoIdentityRegistryFactory<Tx>>(
        &mut self,
        identities: &I,
        identity: CognitoIdentity,
        user_id: UserId,
        email: Email,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .cognito_signup_with_evidence(identities, identity, user_id, email, changed_at)
            .await?
            .intent_id)
    }

    pub(crate) async fn cognito_signup_with_evidence<I: UserCognitoIdentityRegistryFactory<Tx>>(
        &mut self,
        identities: &I,
        identity: CognitoIdentity,
        user_id: UserId,
        email: Email,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        source_key(
            "signup",
            &[identity.issuer.as_str(), identity.subject.as_str()],
        )?;
        let bound_id = identities
            .in_transaction(self.tx)
            .lock_and_find_user_id(&identity)
            .await
            .map_err(map_identity_error)?;
        if bound_id != Some(user_id) {
            return Err(CoordinateMarketingConsentError::UserNotFound);
        }
        self.apply_intent_with_evidence(
            MarketingConsentDecision::CognitoSignup {
                identity,
                user_id,
                email,
            },
            None,
            changed_at,
        )
        .await
    }

    /// Only invoke after a trusted DOI workflow has accepted this proof; not from raw HTTP input.
    pub async fn accepted_double_opt_in(
        &mut self,
        confirmation_id: String,
        email: Email,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .accepted_double_opt_in_with_evidence(confirmation_id, email, changed_at)
            .await?
            .intent_id)
    }

    pub(crate) async fn accepted_double_opt_in_with_evidence(
        &mut self,
        confirmation_id: String,
        email: Email,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        self.accepted_double_opt_in_with_profile_and_evidence(
            confirmation_id,
            email,
            None,
            changed_at,
        )
        .await
    }

    /// Only invoke after a trusted DOI workflow has accepted this proof. The profile
    /// snapshot is persisted with the grant for the asynchronous provider worker.
    pub async fn accepted_double_opt_in_with_profile(
        &mut self,
        confirmation_id: String,
        email: Email,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .accepted_double_opt_in_with_profile_and_evidence(
                confirmation_id,
                email,
                profile_snapshot,
                changed_at,
            )
            .await?
            .intent_id)
    }

    pub(crate) async fn accepted_double_opt_in_with_profile_and_evidence(
        &mut self,
        confirmation_id: String,
        email: Email,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        self.apply_intent_with_evidence(
            MarketingConsentDecision::AcceptedDoubleOptIn {
                confirmation_id,
                email,
            },
            profile_snapshot,
            changed_at,
        )
        .await
    }

    pub async fn user_withdrawal(
        &mut self,
        user_id: UserId,
        email: Email,
        action_id: String,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .user_withdrawal_with_evidence(user_id, email, action_id, changed_at)
            .await?
            .intent_id)
    }

    pub(crate) async fn user_withdrawal_with_evidence(
        &mut self,
        user_id: UserId,
        email: Email,
        action_id: String,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        self.apply_intent_with_evidence(
            MarketingConsentDecision::UserWithdrawal {
                user_id,
                email,
                action_id,
            },
            None,
            changed_at,
        )
        .await
    }

    pub async fn email_only_withdrawal(
        &mut self,
        email: Email,
        action_id: String,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .email_only_withdrawal_with_evidence(email, action_id, changed_at)
            .await?
            .intent_id)
    }

    pub(crate) async fn email_only_withdrawal_with_evidence(
        &mut self,
        email: Email,
        action_id: String,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        self.apply_intent_with_evidence(
            MarketingConsentDecision::EmailOnlyWithdrawal { email, action_id },
            None,
            changed_at,
        )
        .await
    }

    pub async fn user_deletion(
        &mut self,
        user_id: UserId,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
        Ok(self
            .user_deletion_with_evidence(user_id, changed_at)
            .await?
            .intent_id)
    }

    pub(crate) async fn user_deletion_with_evidence(
        &mut self,
        user_id: UserId,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        self.apply_intent_with_evidence(
            MarketingConsentDecision::UserDeletion { user_id },
            None,
            changed_at,
        )
        .await
    }

    pub async fn provider_withdrawal(
        &mut self,
        email: Email,
        accepted_at: OffsetDateTime,
    ) -> Result<(), CoordinateMarketingConsentError> {
        coordinate_marketing_consent_in_transaction(
            self.tx,
            self.intents,
            MarketingConsentDecision::ProviderWithdrawal { email, accepted_at },
            None,
            accepted_at,
        )
        .await?;
        Ok(())
    }

    /// C13 may use this inside its provider-withdrawal transaction. It shares the
    /// recipient lock with confirmation, so an older link cannot restore consent.
    pub async fn invalidate_pending_newsletter_confirmations(
        &mut self,
        email: &Email,
        invalidated_at: OffsetDateTime,
    ) -> Result<(), CoordinateMarketingConsentError> {
        self.intents
            .in_transaction(self.tx)
            .invalidate_newsletter_confirmation_challenges(email, invalidated_at)
            .await?;
        Ok(())
    }

    /// Reserve the DOI source key before locking its recipient challenge row. This
    /// preserves the global source-key -> recipient lock order used by C02 writes.
    pub async fn lock_accepted_double_opt_in_source(
        &mut self,
        confirmation_id: &str,
    ) -> Result<(), CoordinateMarketingConsentError> {
        let key = double_opt_in_source_key(confirmation_id)?;
        self.intents
            .in_transaction(self.tx)
            .lock_source_key(&key)
            .await?;
        Ok(())
    }

    /// Post-send repair for a grant that lost its decision fence while provider I/O
    /// was in flight. The caller owns the transaction and commits before delivery.
    pub async fn repair_raced_grant_if_needed(
        &mut self,
        original: MarketingConsentSyncIntentId,
        changed_at: OffsetDateTime,
    ) -> Result<GrantRaceRepairOutcome, CoordinateMarketingConsentError> {
        let key = source_key("provider-race-repair", &[&original.to_string()])?;
        Ok(self
            .intents
            .in_transaction(self.tx)
            .repair_raced_grant_if_needed(original, &key, changed_at)
            .await?)
    }

    async fn apply_intent_with_evidence(
        &mut self,
        decision: MarketingConsentDecision,
        profile_snapshot: Option<NewsletterProfile>,
        changed_at: OffsetDateTime,
    ) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError> {
        coordinate_marketing_consent_in_transaction(
            self.tx,
            self.intents,
            decision,
            profile_snapshot,
            changed_at,
        )
        .await?
        .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)
    }
}

/// Shared persistence sequence. Private: all public entry points enforce their prerequisites.
async fn coordinate_marketing_consent_in_transaction<Tx, C>(
    tx: &mut Tx,
    intents: &C,
    decision: MarketingConsentDecision,
    profile_snapshot: Option<NewsletterProfile>,
    now: OffsetDateTime,
) -> Result<Option<MarketingConsentDecisionResult>, CoordinateMarketingConsentError>
where
    Tx: Transaction,
    C: MarketingConsentIntentsFactory<Tx>,
{
    let key = decision_source_key(&decision)?;
    let result = match decision {
        MarketingConsentDecision::CognitoSignup { user_id, email, .. } => {
            let mut port = intents.in_transaction(tx);
            let key = key.as_deref().expect("signup has a key");
            if let Some(existing) = port.find_by_source_key(key).await? {
                Some(MarketingConsentDecisionResult {
                    intent_id: replay(
                        existing,
                        ConsentSubject::User(user_id),
                        &email,
                        true,
                        ConsentIntentSource::CognitoSignup,
                    )?,
                    evidence: None,
                })
            } else {
                let user = port
                    .find_user_by_id(user_id)
                    .await?
                    .ok_or(CoordinateMarketingConsentError::UserNotFound)?;
                exact_email(&user.email, &email)?;
                let revision = user
                    .marketing_email_consent_revision
                    .checked_add(1)
                    .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)?;
                let intent = port
                    .record_user_transition(
                        &user,
                        true,
                        ConsentIntentSource::CognitoSignup,
                        key,
                        None,
                        now,
                    )
                    .await?;
                Some(MarketingConsentDecisionResult {
                    intent_id: intent.intent_id,
                    evidence: Some(MarketingConsentEvidence::user_transition(
                        ConsentEvidenceSource::CognitoSignup,
                        ConsentEvidenceAction::Grant,
                        user_id,
                        &email,
                        user.marketing_email_consent,
                        true,
                        intent.intent_id.to_string(),
                        revision,
                        now,
                        "und",
                    )),
                })
            }
        }
        MarketingConsentDecision::AcceptedDoubleOptIn { email, .. } => Some(
            accepted_double_opt_in_in_transaction(
                tx,
                intents,
                &email,
                key.as_deref().expect("confirmation has a key"),
                profile_snapshot,
                now,
            )
            .await?,
        ),
        MarketingConsentDecision::UserWithdrawal { user_id, email, .. } => {
            let mut port = intents.in_transaction(tx);
            let key = key.as_deref().expect("withdrawal has a key");
            if let Some(existing) = port.find_by_source_key(key).await? {
                Some(MarketingConsentDecisionResult {
                    intent_id: replay(
                        existing,
                        ConsentSubject::User(user_id),
                        &email,
                        false,
                        ConsentIntentSource::UserWithdrawal,
                    )?,
                    evidence: None,
                })
            } else {
                let user = port
                    .find_user_by_id(user_id)
                    .await?
                    .ok_or(CoordinateMarketingConsentError::UserNotFound)?;
                exact_email(&user.email, &email)?;
                let revision = user
                    .marketing_email_consent_revision
                    .checked_add(1)
                    .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)?;
                let intent = port
                    .record_user_transition(
                        &user,
                        false,
                        ConsentIntentSource::UserWithdrawal,
                        key,
                        None,
                        now,
                    )
                    .await?;
                Some(MarketingConsentDecisionResult {
                    intent_id: intent.intent_id,
                    evidence: Some(MarketingConsentEvidence::user_transition(
                        ConsentEvidenceSource::UserWithdrawal,
                        ConsentEvidenceAction::Revoke,
                        user_id,
                        &email,
                        user.marketing_email_consent,
                        false,
                        intent.intent_id.to_string(),
                        revision,
                        now,
                        "und",
                    )),
                })
            }
        }
        MarketingConsentDecision::EmailOnlyWithdrawal { email, .. } => {
            let mut port = intents.in_transaction(tx);
            let key = key.as_deref().expect("email withdrawal has a key");
            if let Some(existing) = port.find_by_source_key(key).await? {
                let source = match existing.subject {
                    ConsentSubject::User(_) => ConsentIntentSource::UserWithdrawal,
                    ConsentSubject::EmailOnly => ConsentIntentSource::EmailOnlyWithdrawal,
                };
                if exact_email(&existing.email, &email).is_err()
                    || existing.desired
                    || existing.source != source
                {
                    return Err(CoordinateMarketingConsentError::SourceKeyConflict);
                }
                Some(MarketingConsentDecisionResult {
                    intent_id: existing.intent_id,
                    evidence: None,
                })
            } else if let Some(user) = port.find_user_by_email(&email).await? {
                exact_email(&user.email, &email)?;
                let revision = user
                    .marketing_email_consent_revision
                    .checked_add(1)
                    .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)?;
                let intent = port
                    .record_user_transition(
                        &user,
                        false,
                        ConsentIntentSource::UserWithdrawal,
                        key,
                        None,
                        now,
                    )
                    .await?;
                Some(MarketingConsentDecisionResult {
                    intent_id: intent.intent_id,
                    evidence: Some(MarketingConsentEvidence::user_transition(
                        ConsentEvidenceSource::UserWithdrawal,
                        ConsentEvidenceAction::Revoke,
                        user.user_id,
                        &email,
                        user.marketing_email_consent,
                        false,
                        intent.intent_id.to_string(),
                        revision,
                        now,
                        "und",
                    )),
                })
            } else {
                let intent = port
                    .record_email_only_intent(
                        &email,
                        false,
                        ConsentIntentSource::EmailOnlyWithdrawal,
                        key,
                        None,
                        now,
                    )
                    .await?;
                Some(MarketingConsentDecisionResult {
                    intent_id: intent.intent_id,
                    evidence: Some(MarketingConsentEvidence::email_only(
                        ConsentEvidenceSource::UserWithdrawal,
                        ConsentEvidenceAction::Revoke,
                        &email,
                        intent.intent_id.to_string(),
                        now,
                        "und",
                    )),
                })
            }
        }

        MarketingConsentDecision::UserDeletion { user_id } => {
            let mut port = intents.in_transaction(tx);
            let key = key.as_deref().expect("deletion has a key");
            if let Some(existing) = port.find_by_source_key(key).await? {
                if existing.subject != ConsentSubject::User(user_id)
                    || existing.desired
                    || existing.source != ConsentIntentSource::UserDeletion
                {
                    return Err(CoordinateMarketingConsentError::SourceKeyConflict);
                }
                Some(MarketingConsentDecisionResult {
                    intent_id: existing.intent_id,
                    evidence: None,
                })
            } else {
                let user = port
                    .find_user_by_id(user_id)
                    .await?
                    .ok_or(CoordinateMarketingConsentError::UserNotFound)?;
                let revision = user
                    .marketing_email_consent_revision
                    .checked_add(1)
                    .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)?;
                let intent = port.record_user_deletion(&user, key, now).await?;
                Some(MarketingConsentDecisionResult {
                    intent_id: intent.intent_id,
                    evidence: Some(
                        MarketingConsentEvidence::user_transition(
                            ConsentEvidenceSource::AccountDeleted,
                            ConsentEvidenceAction::Revoke,
                            user_id,
                            &user.email,
                            user.marketing_email_consent,
                            false,
                            intent.intent_id.to_string(),
                            revision,
                            now,
                            "und",
                        )
                        .with_wording_reference("account-deletion-flow:v1"),
                    ),
                })
            }
        }
        MarketingConsentDecision::ProviderWithdrawal { email, accepted_at } => {
            let mut port = intents.in_transaction(tx);
            port.lock_recipient(&email).await?;
            if let Some(user) = port.find_user_by_email(&email).await? {
                exact_email(&user.email, &email)?;
                port.apply_provider_withdrawal(&user, accepted_at).await?;
            }
            port.cancel_provider_backsync(&email).await?;
            port.invalidate_newsletter_confirmation_challenges(&email, accepted_at)
                .await?;
            None
        }
    };
    Ok(result)
}

async fn accepted_double_opt_in_in_transaction<Tx, C>(
    tx: &mut Tx,
    intents: &C,
    email: &Email,
    key: &str,
    profile_snapshot: Option<NewsletterProfile>,
    now: OffsetDateTime,
) -> Result<MarketingConsentDecisionResult, CoordinateMarketingConsentError>
where
    Tx: Transaction,
    C: MarketingConsentIntentsFactory<Tx>,
{
    let mut port = intents.in_transaction(tx);
    if let Some(existing) = port.find_by_source_key(key).await? {
        if exact_email(&existing.email, email).is_err()
            || !existing.desired
            || existing.source != ConsentIntentSource::AuraDoubleOptIn
            || existing.profile_snapshot.as_deref() != profile_snapshot.as_ref()
        {
            return Err(CoordinateMarketingConsentError::SourceKeyConflict);
        }
        Ok(MarketingConsentDecisionResult {
            intent_id: existing.intent_id,
            evidence: None,
        })
    } else if let Some(user) = port.find_user_by_email(email).await? {
        exact_email(&user.email, email)?;
        let revision = user
            .marketing_email_consent_revision
            .checked_add(1)
            .ok_or(CoordinateMarketingConsentError::InvalidPersistedState)?;
        let intent = port
            .record_user_transition(
                &user,
                true,
                ConsentIntentSource::AuraDoubleOptIn,
                key,
                profile_snapshot,
                now,
            )
            .await?;
        Ok(MarketingConsentDecisionResult {
            intent_id: intent.intent_id,
            evidence: Some(MarketingConsentEvidence::user_transition(
                ConsentEvidenceSource::AuraDoubleOptIn,
                ConsentEvidenceAction::Grant,
                user.user_id,
                email,
                user.marketing_email_consent,
                true,
                intent.intent_id.to_string(),
                revision,
                now,
                "und",
            )),
        })
    } else {
        let intent = port
            .record_email_only_intent(
                email,
                true,
                ConsentIntentSource::AuraDoubleOptIn,
                key,
                profile_snapshot,
                now,
            )
            .await?;
        Ok(MarketingConsentDecisionResult {
            intent_id: intent.intent_id,
            evidence: Some(MarketingConsentEvidence::email_only(
                ConsentEvidenceSource::AuraDoubleOptIn,
                ConsentEvidenceAction::Grant,
                email,
                intent.intent_id.to_string(),
                now,
                "und",
            )),
        })
    }
}

fn decision_source_key(
    decision: &MarketingConsentDecision,
) -> Result<Option<String>, CoordinateMarketingConsentError> {
    Ok(match decision {
        MarketingConsentDecision::CognitoSignup { identity, .. } => Some(source_key(
            "signup",
            &[identity.issuer.as_str(), identity.subject.as_str()],
        )?),
        MarketingConsentDecision::AcceptedDoubleOptIn {
            confirmation_id, ..
        } => Some(double_opt_in_source_key(confirmation_id)?),
        MarketingConsentDecision::UserWithdrawal { action_id, .. } => {
            Some(source_key("withdrawal", &[action_id])?)
        }
        MarketingConsentDecision::EmailOnlyWithdrawal { action_id, .. } => {
            Some(source_key("email-withdrawal", &[action_id])?)
        }

        MarketingConsentDecision::UserDeletion { user_id } => {
            Some(user_deletion_source_key(*user_id))
        }
        MarketingConsentDecision::ProviderWithdrawal { .. } => None,
    })
}

fn double_opt_in_source_key(
    confirmation_id: &str,
) -> Result<String, CoordinateMarketingConsentError> {
    source_key("doi", &[confirmation_id])
}

/// Stable across retries, actors and entry points.
pub(crate) fn user_deletion_source_key(user_id: UserId) -> String {
    format!("user-deletion:{user_id}")
}

fn exact_email(actual: &Email, expected: &Email) -> Result<(), CoordinateMarketingConsentError> {
    let actual: &str = actual.as_ref();
    let expected: &str = expected.as_ref();
    if actual == expected {
        Ok(())
    } else {
        Err(CoordinateMarketingConsentError::EmailMismatch)
    }
}

fn replay(
    existing: ConsentIntent,
    subject: ConsentSubject,
    email: &Email,
    desired: bool,
    source: ConsentIntentSource,
) -> Result<MarketingConsentSyncIntentId, CoordinateMarketingConsentError> {
    if existing.subject != subject
        || exact_email(&existing.email, email).is_err()
        || existing.desired != desired
        || existing.source != source
    {
        return Err(CoordinateMarketingConsentError::SourceKeyConflict);
    }
    Ok(existing.intent_id)
}

fn source_key(kind: &str, values: &[&str]) -> Result<String, CoordinateMarketingConsentError> {
    let mut digest = Sha256::new();
    digest.update(b"aura:marketing-consent:source:v1\0");
    digest.update(kind.as_bytes());
    for value in values {
        if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control) {
            return Err(CoordinateMarketingConsentError::InvalidIdentity);
        }
        digest.update((value.len() as u32).to_be_bytes());
        digest.update(value.as_bytes());
    }
    Ok(format!("{kind}:{}", hex_digest(digest.finalize())))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(64);
    for byte in bytes.as_ref() {
        write!(&mut result, "{byte:02x}").expect("writing to String cannot fail");
    }
    result
}

fn map_identity_error(error: UserCognitoIdentityRegistryError) -> CoordinateMarketingConsentError {
    match error {
        UserCognitoIdentityRegistryError::TemporarilyUnavailable { .. } => {
            CoordinateMarketingConsentError::TemporarilyUnavailable
        }
        UserCognitoIdentityRegistryError::InvalidPersistedIdentity { .. } => {
            CoordinateMarketingConsentError::InvalidPersistedState
        }
        _ => CoordinateMarketingConsentError::ConcurrencyConflict,
    }
}

impl From<MarketingConsentIntentError> for CoordinateMarketingConsentError {
    fn from(error: MarketingConsentIntentError) -> Self {
        match error {
            MarketingConsentIntentError::ConcurrencyConflict => Self::ConcurrencyConflict,
            MarketingConsentIntentError::SourceKeyConflict => Self::SourceKeyConflict,
            MarketingConsentIntentError::RegisteredEmail => Self::EmailMismatch,
            MarketingConsentIntentError::InvalidInput => Self::InvalidIdentity,
            MarketingConsentIntentError::InvalidPersistedState => Self::InvalidPersistedState,
            MarketingConsentIntentError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ConsentUser, MarketingConsentIntents, UserStorageVersion};

    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::TransactionError;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Default)]
    struct State {
        calls: Vec<&'static str>,
        user: Option<ConsentUser>,
        intent: Option<ConsentIntent>,
        identity_user: Option<UserId>,
        conflict: bool,
        fail_commit: bool,
        accepted_at: Option<OffsetDateTime>,
    }
    type Shared = Arc<Mutex<State>>;
    fn locked(state: &Shared) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    struct Work(Shared);
    struct Tx(Shared);
    struct TrackedTx {
        inner: Tx,
        committed: bool,
    }
    impl TrackedTx {
        fn new(state: Shared) -> Self {
            Self {
                inner: Tx(state),
                committed: false,
            }
        }
    }
    impl Drop for TrackedTx {
        fn drop(&mut self) {
            if !self.committed {
                locked(&self.inner.0).calls.push("rollback");
            }
        }
    }
    #[async_trait::async_trait]
    impl Transaction for TrackedTx {
        async fn commit(mut self) -> Result<(), TransactionError> {
            locked(&self.inner.0).calls.push("commit");
            self.committed = true;
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = locked(&self.0);
            state.calls.push("commit");
            if state.fail_commit {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for Work {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            locked(&self.0).calls.push("begin");
            Ok(Tx(self.0.clone()))
        }
    }
    struct Identities(Shared);
    struct Registry(Shared);
    impl UserCognitoIdentityRegistryFactory<Tx> for Identities {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut Tx,
        ) -> impl UserCognitoIdentityRegistry + 'tx {
            Registry(self.0.clone())
        }
    }
    impl UserCognitoIdentityRegistryFactory<TrackedTx> for Identities {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TrackedTx,
        ) -> impl UserCognitoIdentityRegistry + 'tx {
            <Self as UserCognitoIdentityRegistryFactory<Tx>>::in_transaction(self, &mut tx.inner)
        }
    }
    #[async_trait::async_trait]
    impl UserCognitoIdentityRegistry for Registry {
        async fn lock_and_find_user_id(
            &mut self,
            _: &CognitoIdentity,
        ) -> Result<Option<UserId>, UserCognitoIdentityRegistryError> {
            let mut state = locked(&self.0);
            state.calls.push("identity");
            Ok(state.identity_user)
        }
        async fn find_by_user_id(
            &mut self,
            _: UserId,
        ) -> Result<Option<CognitoIdentity>, UserCognitoIdentityRegistryError> {
            unreachable!()
        }
        async fn bind(
            &mut self,
            _: &CognitoIdentity,
            _: UserId,
        ) -> Result<(), UserCognitoIdentityRegistryError> {
            unreachable!()
        }
    }
    struct Intents(Shared);
    struct Port(Shared);
    impl MarketingConsentIntentsFactory<Tx> for Intents {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut Tx) -> impl MarketingConsentIntents + 'tx {
            Port(self.0.clone())
        }
    }
    impl MarketingConsentIntentsFactory<TrackedTx> for Intents {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut TrackedTx,
        ) -> impl MarketingConsentIntents + 'tx {
            <Self as MarketingConsentIntentsFactory<Tx>>::in_transaction(self, &mut tx.inner)
        }
    }
    #[async_trait::async_trait]
    impl MarketingConsentIntents for Port {
        async fn lock_recipient(&mut self, _: &Email) -> Result<(), MarketingConsentIntentError> {
            locked(&self.0).calls.push("lock_recipient");
            Ok(())
        }

        async fn lock_source_key(&mut self, _: &str) -> Result<(), MarketingConsentIntentError> {
            locked(&self.0).calls.push("lock_source");
            Ok(())
        }

        async fn find_by_source_key(
            &mut self,
            key: &str,
        ) -> Result<Option<ConsentIntent>, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("source");
            Ok(state
                .intent
                .as_ref()
                .filter(|intent| intent.source_key == key)
                .cloned())
        }
        async fn find_user_by_id(
            &mut self,
            id: UserId,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("by_id");
            Ok(state
                .user
                .as_ref()
                .filter(|user| user.user_id == id)
                .cloned())
        }
        async fn find_user_by_email(
            &mut self,
            email: &Email,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("by_email");
            Ok(state
                .user
                .as_ref()
                .filter(|user| user.email == *email)
                .cloned())
        }
        async fn record_user_transition(
            &mut self,
            user: &ConsentUser,
            desired: bool,
            source: ConsentIntentSource,
            key: &str,
            profile_snapshot: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("transition");
            if state.conflict {
                return Err(MarketingConsentIntentError::ConcurrencyConflict);
            }
            let intent = intent(
                key,
                ConsentSubject::User(user.user_id),
                &user.email,
                desired,
                source,
            );
            let mut intent = intent;
            intent.profile_snapshot = profile_snapshot.map(Box::new);
            state.intent = Some(intent.clone());
            Ok(intent)
        }
        async fn record_email_only_intent(
            &mut self,
            email: &Email,
            desired: bool,
            source: ConsentIntentSource,
            key: &str,
            profile_snapshot: Option<NewsletterProfile>,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("email_only");
            let mut intent = intent(key, ConsentSubject::EmailOnly, email, desired, source);
            intent.profile_snapshot = profile_snapshot.map(Box::new);
            state.intent = Some(intent.clone());
            Ok(intent)
        }
        async fn apply_provider_withdrawal(
            &mut self,
            _: &ConsentUser,
            accepted_at: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("backsync");
            state.accepted_at = Some(accepted_at);
            Ok(())
        }
        async fn cancel_provider_backsync(
            &mut self,
            _: &Email,
        ) -> Result<(), MarketingConsentIntentError> {
            locked(&self.0).calls.push("cancel");
            Ok(())
        }

        async fn invalidate_newsletter_confirmation_challenges(
            &mut self,
            _: &Email,
            _: OffsetDateTime,
        ) -> Result<(), MarketingConsentIntentError> {
            locked(&self.0).calls.push("invalidate_challenges");
            Ok(())
        }

        async fn repair_raced_grant_if_needed(
            &mut self,
            _: MarketingConsentSyncIntentId,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<GrantRaceRepairOutcome, MarketingConsentIntentError> {
            Ok(GrantRaceRepairOutcome::NoRepairNeeded)
        }

        async fn record_user_deletion(
            &mut self,
            user: &ConsentUser,
            key: &str,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            let mut state = locked(&self.0);
            state.calls.push("deletion");
            let intent = intent(
                key,
                ConsentSubject::User(user.user_id),
                &user.email,
                false,
                ConsentIntentSource::UserDeletion,
            );
            state.intent = Some(intent.clone());
            Ok(intent)
        }
    }
    fn intent(
        key: &str,
        subject: ConsentSubject,
        email: &Email,
        desired: bool,
        source: ConsentIntentSource,
    ) -> ConsentIntent {
        ConsentIntent {
            intent_id: MarketingConsentSyncIntentId::new(),
            source_key: key.to_owned(),
            subject,
            email: email.clone(),
            profile_snapshot: None,

            desired,
            source,
        }
    }
    fn email(value: &str) -> Email {
        Email::try_from(value).unwrap()
    }
    fn identity() -> CognitoIdentity {
        CognitoIdentity {
            issuer: "https://issuer.test/pool".try_into().unwrap(),
            subject: "opaque-subject".try_into().unwrap(),
        }
    }
    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn handler(state: &Shared) -> CoordinateMarketingConsentHandler<Work, Identities, Intents> {
        CoordinateMarketingConsentHandler::new(
            Work(state.clone()),
            Identities(state.clone()),
            Intents(state.clone()),
        )
    }
    fn with_user() -> (Shared, UserId) {
        let id = UserId::new();
        let state = Arc::new(Mutex::new(State {
            user: Some(ConsentUser {
                user_id: id,
                email: email("person@example.test"),
                version: UserStorageVersion::INITIAL,
                marketing_email_consent: false,
                marketing_email_consent_revision: 0,
                marketing_email_consent_changed_at: None,
            }),
            identity_user: Some(id),
            ..Default::default()
        }));
        (state, id)
    }

    #[derive(Clone)]
    struct CommitAwareMakeWriter {
        state: Shared,
        output: Arc<Mutex<Vec<u8>>>,
    }

    struct CommitAwareWriter {
        state: Shared,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl<'writer> MakeWriter<'writer> for CommitAwareMakeWriter {
        type Writer = CommitAwareWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            CommitAwareWriter {
                state: self.state.clone(),
                output: self.output.clone(),
            }
        }
    }

    impl Write for CommitAwareWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            assert_eq!(locked(&self.state).calls.last(), Some(&"commit"));
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn committed_signup_logs_exact_evidence_after_commit_and_failure_logs_nothing() {
        let (state, id) = with_user();
        locked(&state).user.as_mut().unwrap().email = email("private.recipient@example.test");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommitAwareMakeWriter {
                state: state.clone(),
                output: output.clone(),
            })
            .finish();
        let decision = || MarketingConsentDecision::CognitoSignup {
            identity: identity(),
            user_id: id,
            email: email("private.recipient@example.test"),
        };

        tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(handler(&state).execute(&context(Principal::System), decision()))
        })
        .unwrap();
        let line = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let record: serde_json::Value = serde_json::from_str(&line).unwrap();
        let fields = &record["fields"];
        assert_eq!(fields["event"], "marketing_consent.evidence.v1");
        assert_eq!(fields["consent_purpose"], "EMAIL_MARKETING");
        assert_eq!(fields["consent_source"], "COGNITO_SIGNUP");
        assert_eq!(fields["consent_action"], "GRANT");
        assert_eq!(fields["subject_kind"], "USER");
        assert_eq!(fields["user_id"], id.to_string());
        assert!(
            fields["consent_decision_id"]
                .as_str()
                .unwrap()
                .starts_with("mci_")
        );
        assert_eq!(fields["recipient_fingerprint"].as_str().unwrap().len(), 64);
        assert_eq!(fields["previous_consent"], false);
        assert_eq!(fields["current_consent"], true);
        assert_eq!(fields["consent_revision"], 1);
        assert!(
            fields["consent_effective_at_utc"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        assert!(
            fields["consent_recorded_at_utc"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        assert_eq!(fields["consent_wording_reference"], "not-recorded");
        assert_eq!(fields["consent_wording_locale"], "und");
        let sha = fields["backend_release_sha"].as_str().unwrap();
        assert_eq!(sha.len(), 40);
        assert!(sha.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(fields["request_id"], "request");
        assert_eq!(fields["correlation_id"], "correlation");
        for prohibited in [
            "private.recipient@example.test",
            "raw_token",
            "token_hash",
            "signature",
            "password",
            "provider_payload",
            "rendered_email",
        ] {
            assert!(!line.contains(prohibited));
        }
        assert!(locked(&state).calls.ends_with(&["transition", "commit"]));

        locked(&state).calls.clear();
        output.lock().unwrap().clear();
        let replay_subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommitAwareMakeWriter {
                state: state.clone(),
                output: output.clone(),
            })
            .finish();
        tracing::subscriber::with_default(replay_subscriber, || {
            runtime.block_on(handler(&state).execute(&context(Principal::System), decision()))
        })
        .unwrap();
        assert!(output.lock().unwrap().is_empty());

        locked(&state).calls.clear();
        if let Some(user) = locked(&state).user.as_mut() {
            user.marketing_email_consent = true;
            user.marketing_email_consent_revision = 1;
            user.marketing_email_consent_changed_at = Some(OffsetDateTime::UNIX_EPOCH);
        }
        output.lock().unwrap().clear();
        let revoke_subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommitAwareMakeWriter {
                state: state.clone(),
                output: output.clone(),
            })
            .finish();
        tracing::subscriber::with_default(revoke_subscriber, || {
            runtime.block_on(handler(&state).execute(
                &context(Principal::User(id)),
                MarketingConsentDecision::UserWithdrawal {
                    user_id: id,
                    email: email("private.recipient@example.test"),
                    action_id: "safe-action-1".to_owned(),
                },
            ))
        })
        .unwrap();
        let revoke_line = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        let revoke_record: serde_json::Value = serde_json::from_str(&revoke_line).unwrap();
        let revoke_fields = &revoke_record["fields"];
        assert_eq!(revoke_fields["consent_source"], "USER_WITHDRAWAL");
        assert_eq!(revoke_fields["consent_action"], "REVOKE");
        assert_eq!(revoke_fields["previous_consent"], true);
        assert_eq!(revoke_fields["current_consent"], false);
        assert_eq!(revoke_fields["consent_wording_reference"], "not-recorded");
        assert_eq!(revoke_fields["consent_wording_locale"], "und");
        assert!(locked(&state).calls.ends_with(&["transition", "commit"]));

        let (failed_state, failed_id) = with_user();
        locked(&failed_state).user.as_mut().unwrap().email =
            email("private.recipient@example.test");
        locked(&failed_state).conflict = true;
        let failed_output = Arc::new(Mutex::new(Vec::new()));
        let failed_subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommitAwareMakeWriter {
                state: failed_state.clone(),
                output: failed_output.clone(),
            })
            .finish();
        let failed_decision = || MarketingConsentDecision::CognitoSignup {
            identity: identity(),
            user_id: failed_id,
            email: email("private.recipient@example.test"),
        };
        assert!(
            tracing::subscriber::with_default(failed_subscriber, || {
                runtime.block_on(
                    handler(&failed_state).execute(&context(Principal::System), failed_decision()),
                )
            },)
            .is_err()
        );
        assert!(failed_output.lock().unwrap().is_empty());
        assert!(!locked(&failed_state).calls.contains(&"commit"));

        let (commit_failed_state, commit_failed_id) = with_user();
        locked(&commit_failed_state).user.as_mut().unwrap().email =
            email("private.recipient@example.test");
        locked(&commit_failed_state).fail_commit = true;
        let commit_failed_output = Arc::new(Mutex::new(Vec::new()));
        let commit_failed_subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_writer(CommitAwareMakeWriter {
                state: commit_failed_state.clone(),
                output: commit_failed_output.clone(),
            })
            .finish();
        let commit_failed_decision = || MarketingConsentDecision::CognitoSignup {
            identity: identity(),
            user_id: commit_failed_id,
            email: email("private.recipient@example.test"),
        };
        assert!(
            tracing::subscriber::with_default(commit_failed_subscriber, || {
                runtime.block_on(
                    handler(&commit_failed_state)
                        .execute(&context(Principal::System), commit_failed_decision()),
                )
            },)
            .is_err()
        );
        assert!(commit_failed_output.lock().unwrap().is_empty());
        assert!(
            locked(&commit_failed_state)
                .calls
                .ends_with(&["transition", "commit"])
        );
    }

    #[test]
    fn both_email_withdrawal_paths_do_not_claim_signup_wording() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        for registered in [true, false] {
            let (state, _) = with_user();
            if registered {
                locked(&state)
                    .user
                    .as_mut()
                    .unwrap()
                    .marketing_email_consent = true;
            } else {
                locked(&state).user = None;
            }
            let output = Arc::new(Mutex::new(Vec::new()));
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_ansi(false)
                .with_writer(CommitAwareMakeWriter {
                    state: state.clone(),
                    output: output.clone(),
                })
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                runtime.block_on(handler(&state).execute(
                    &context(Principal::System),
                    MarketingConsentDecision::EmailOnlyWithdrawal {
                        email: email("person@example.test"),
                        action_id: format!("withdrawal-{registered}"),
                    },
                ))
            })
            .unwrap();
            let line = String::from_utf8(output.lock().unwrap().clone()).unwrap();
            let record: serde_json::Value = serde_json::from_str(&line).unwrap();
            let fields = &record["fields"];
            assert_eq!(fields["consent_source"], "USER_WITHDRAWAL");
            assert_eq!(fields["consent_action"], "REVOKE");
            assert_eq!(fields["consent_wording_reference"], "not-recorded");
            assert_eq!(fields["consent_wording_locale"], "und");
            assert_eq!(
                fields["subject_kind"],
                if registered { "USER" } else { "EMAIL_ONLY" }
            );
        }
    }

    #[test]
    fn proof_keys_are_domain_separated_and_unambiguous() {
        assert_ne!(
            source_key("signup", &["ab", "c"]).unwrap(),
            source_key("signup", &["a", "bc"]).unwrap()
        );
        assert_ne!(
            source_key("doi", &["proof"]).unwrap(),
            source_key("signup", &["proof"]).unwrap()
        );
        assert!(source_key("doi", &["\n"]).is_err());
    }
    #[tokio::test]
    async fn signup_replay_after_withdrawal_does_not_regrant() {
        let (state, id) = with_user();
        let decide = || MarketingConsentDecision::CognitoSignup {
            identity: identity(),
            user_id: id,
            email: email("person@example.test"),
        };
        let first = handler(&state)
            .execute(&context(Principal::System), decide())
            .await
            .unwrap();
        locked(&state).calls.clear();
        let replay = handler(&state)
            .execute(&context(Principal::System), decide())
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            locked(&state).calls,
            ["begin", "identity", "source", "commit"]
        );
    }
    #[tokio::test]
    async fn signup_rejects_unbound_identity_or_wrong_exact_email() {
        let (state, id) = with_user();
        locked(&state).identity_user = None;
        let decision = || MarketingConsentDecision::CognitoSignup {
            identity: identity(),
            user_id: id,
            email: email("other@example.test"),
        };
        assert!(matches!(
            handler(&state)
                .execute(&context(Principal::System), decision())
                .await,
            Err(CoordinateMarketingConsentError::UserNotFound)
        ));
        locked(&state).identity_user = Some(id);
        assert!(matches!(
            handler(&state)
                .execute(&context(Principal::System), decision())
                .await,
            Err(CoordinateMarketingConsentError::EmailMismatch)
        ));
        assert!(!locked(&state).calls.contains(&"transition"));
        assert!(!locked(&state).calls.contains(&"commit"));
    }
    #[tokio::test]
    async fn confirmation_resolves_registered_email_or_standalone_without_actor_id() {
        let (state, _) = with_user();
        let decide = |proof: &str| MarketingConsentDecision::AcceptedDoubleOptIn {
            confirmation_id: proof.into(),
            email: email("person@example.test"),
        };
        handler(&state)
            .execute(&context(Principal::System), decide("proof-1"))
            .await
            .unwrap();
        assert_eq!(
            locked(&state).calls,
            ["begin", "source", "by_email", "transition", "commit"]
        );
        locked(&state).calls.clear();
        locked(&state).user = None;
        handler(&state)
            .execute(&context(Principal::System), decide("proof-2"))
            .await
            .unwrap();
        assert_eq!(
            locked(&state).calls,
            ["begin", "source", "by_email", "email_only", "commit"]
        );
    }
    #[tokio::test]
    async fn standalone_withdrawal_resolves_exact_registered_email() {
        let (state, _) = with_user();
        handler(&state)
            .execute(
                &context(Principal::System),
                MarketingConsentDecision::EmailOnlyWithdrawal {
                    email: email("person@example.test"),
                    action_id: "opt-out-1".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            locked(&state).calls,
            ["begin", "source", "by_email", "transition", "commit"]
        );
        assert_eq!(
            locked(&state).intent.as_ref().unwrap().source,
            ConsentIntentSource::UserWithdrawal
        );
    }

    #[tokio::test]
    async fn accepted_confirmation_replay_cannot_regrant_or_change_target() {
        let (state, _) = with_user();
        let proof = |address: &str| MarketingConsentDecision::AcceptedDoubleOptIn {
            confirmation_id: "confirmation-1".into(),
            email: email(address),
        };
        let first = handler(&state)
            .execute(&context(Principal::System), proof("person@example.test"))
            .await
            .unwrap();
        locked(&state).calls.clear();
        let replay = handler(&state)
            .execute(&context(Principal::System), proof("person@example.test"))
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(locked(&state).calls, ["begin", "source", "commit"]);
        assert!(matches!(
            handler(&state)
                .execute(&context(Principal::System), proof("different@example.test"))
                .await,
            Err(CoordinateMarketingConsentError::SourceKeyConflict)
        ));
        assert_eq!(
            locked(&state)
                .calls
                .iter()
                .filter(|call| **call == "transition")
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn provider_withdrawal_has_no_outbound_echo() {
        let (state, _) = with_user();
        let accepted_at = OffsetDateTime::UNIX_EPOCH;
        handler(&state)
            .execute(
                &context(Principal::System),
                MarketingConsentDecision::ProviderWithdrawal {
                    email: email("person@example.test"),
                    accepted_at,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            locked(&state).calls,
            [
                "begin",
                "lock_recipient",
                "by_email",
                "backsync",
                "cancel",
                "invalidate_challenges",
                "commit"
            ]
        );
        assert_eq!(locked(&state).accepted_at, Some(accepted_at));
    }
    #[tokio::test]
    async fn deletion_uses_atomic_port_without_new_grant() {
        let (state, id) = with_user();
        handler(&state)
            .execute(
                &context(Principal::System),
                MarketingConsentDecision::UserDeletion { user_id: id },
            )
            .await
            .unwrap();
        assert_eq!(
            locked(&state).calls,
            ["begin", "source", "by_id", "deletion", "commit"]
        );
        assert_eq!(user_deletion_source_key(id), format!("user-deletion:{id}"));
        assert_eq!(
            locked(&state).intent.as_ref().unwrap().source_key,
            user_deletion_source_key(id)
        );
    }

    #[tokio::test]
    async fn transaction_bound_deletion_replays_without_begin_or_commit() {
        let (state, id) = with_user();
        let mut tx = Tx(state.clone());
        let intents = Intents(state.clone());
        let mut coordinator = MarketingConsentCoordinator::new(&mut tx, &intents);
        let first = coordinator
            .user_deletion(id, OffsetDateTime::UNIX_EPOCH)
            .await
            .unwrap();
        locked(&state).user = None;
        let replay = coordinator
            .user_deletion(id, OffsetDateTime::now_utc())
            .await
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(
            locked(&state).calls,
            ["source", "by_id", "deletion", "source"]
        );
    }
    #[tokio::test]
    async fn trusted_signup_composes_with_outer_work_in_one_commit_and_rolls_back_on_error() {
        let (state, id) = with_user();
        let mut tx = TrackedTx::new(state.clone());
        locked(&state).calls.push("outer_before");
        let intents = Intents(state.clone());
        let identities = Identities(state.clone());
        MarketingConsentCoordinator::new(&mut tx, &intents)
            .cognito_signup(
                &identities,
                identity(),
                id,
                email("person@example.test"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await
            .unwrap();
        locked(&state).calls.push("outer_after");
        tx.commit().await.unwrap();
        assert_eq!(
            locked(&state).calls,
            [
                "outer_before",
                "identity",
                "source",
                "by_id",
                "transition",
                "outer_after",
                "commit"
            ]
        );

        locked(&state).calls.clear();
        locked(&state).conflict = true;
        let mut tx = TrackedTx::new(state.clone());
        locked(&state).calls.push("outer_before");
        let mut new_identity = identity();
        new_identity.subject = "new-proof".try_into().unwrap();
        let result = MarketingConsentCoordinator::new(&mut tx, &intents)
            .cognito_signup(
                &identities,
                new_identity,
                id,
                email("person@example.test"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await;
        assert!(matches!(
            result,
            Err(CoordinateMarketingConsentError::ConcurrencyConflict)
        ));
        drop(tx);
        assert_eq!(
            locked(&state).calls,
            [
                "outer_before",
                "identity",
                "source",
                "by_id",
                "transition",
                "rollback"
            ]
        );
    }

    #[tokio::test]
    async fn transaction_bound_signup_rechecks_binding_and_exact_email_before_grant() {
        let (state, id) = with_user();
        let intents = Intents(state.clone());
        let identities = Identities(state.clone());
        locked(&state).identity_user = None;
        let mut tx = TrackedTx::new(state.clone());
        let result = MarketingConsentCoordinator::new(&mut tx, &intents)
            .cognito_signup(
                &identities,
                identity(),
                id,
                email("person@example.test"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await;
        assert!(matches!(
            result,
            Err(CoordinateMarketingConsentError::UserNotFound)
        ));
        drop(tx);
        assert_eq!(locked(&state).calls, ["identity", "rollback"]);
        assert!(locked(&state).intent.is_none());

        locked(&state).calls.clear();
        locked(&state).identity_user = Some(id);
        let mut tx = TrackedTx::new(state.clone());
        let result = MarketingConsentCoordinator::new(&mut tx, &intents)
            .cognito_signup(
                &identities,
                identity(),
                id,
                email("wrong@example.test"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await;
        assert!(matches!(
            result,
            Err(CoordinateMarketingConsentError::EmailMismatch)
        ));
        drop(tx);
        assert_eq!(
            locked(&state).calls,
            ["identity", "source", "by_id", "rollback"]
        );
        assert!(locked(&state).intent.is_none());
    }

    #[tokio::test]
    async fn trusted_anonymous_doi_is_composable_without_an_http_principal() {
        let (state, _) = with_user();
        let mut tx = TrackedTx::new(state.clone());
        let intents = Intents(state.clone());
        locked(&state).calls.push("outer_before");
        MarketingConsentCoordinator::new(&mut tx, &intents)
            .accepted_double_opt_in(
                "accepted-proof".into(),
                email("person@example.test"),
                OffsetDateTime::UNIX_EPOCH,
            )
            .await
            .unwrap();
        locked(&state).calls.push("outer_after");
        tx.commit().await.unwrap();
        assert_eq!(
            locked(&state).calls,
            [
                "outer_before",
                "source",
                "by_email",
                "transition",
                "outer_after",
                "commit"
            ]
        );
    }

    #[tokio::test]
    async fn unauthorized_and_conflicting_decisions_never_commit() {
        let (state, id) = with_user();
        assert!(matches!(
            handler(&state)
                .execute(
                    &context(Principal::Anonymous),
                    MarketingConsentDecision::ProviderWithdrawal {
                        email: email("person@example.test"),
                        accepted_at: OffsetDateTime::UNIX_EPOCH,
                    }
                )
                .await,
            Err(CoordinateMarketingConsentError::Forbidden)
        ));
        assert!(locked(&state).calls.is_empty());
        assert!(matches!(
            handler(&state)
                .execute(
                    &context(Principal::Anonymous),
                    MarketingConsentDecision::AcceptedDoubleOptIn {
                        confirmation_id: "unverified-assertion".into(),
                        email: email("person@example.test"),
                    },
                )
                .await,
            Err(CoordinateMarketingConsentError::Forbidden)
        ));
        assert!(locked(&state).calls.is_empty());
        locked(&state).conflict = true;
        assert!(matches!(
            handler(&state)
                .execute(
                    &context(Principal::User(id)),
                    MarketingConsentDecision::UserWithdrawal {
                        user_id: id,
                        email: email("person@example.test"),
                        action_id: "withdrawal-1".into()
                    }
                )
                .await,
            Err(CoordinateMarketingConsentError::ConcurrencyConflict)
        ));
        assert!(!locked(&state).calls.contains(&"commit"));
    }
}
