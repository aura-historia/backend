use crate::ports::{
    MarketingConsentIntentError, MarketingConsentIntentsFactory, UserAdminMutationGuard,
    UserAdminMutationGuardFactory, UserAdminReadError, UserAdminReaderFactory,
    UserAdminRemovalDecision,
};
use crate::use_cases::authorization::{RequireAdminActorError, require_admin_actor};
use crate::use_cases::commands::coordinate_marketing_consent::{
    CoordinateMarketingConsentError, MarketingConsentCoordinator,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteUserCommand {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteUserResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteUserError {
    #[error("authenticated actor required to delete user")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("cannot remove the last administrator")]
    LastAdminProtected,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user email already exists")]
    EmailConflict {
        #[source]
        source: BoxError,
    },
    #[error("user stripe customer already exists")]
    StripeCustomerConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary user persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted user state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal user persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin delete user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit delete user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait DeleteUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteUserCommand,
    ) -> Result<DeleteUserResult, DeleteUserError>;
}

pub struct DeleteUserHandler<U, C, A> {
    unit_of_work: U,
    consent: C,
    admin_reader: A,
    admin_only: bool,
}

impl<U, C, A> DeleteUserHandler<U, C, A> {
    pub fn new(unit_of_work: U, consent: C, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            consent,
            admin_reader,
            admin_only: false,
        }
    }

    pub fn new_admin_only(unit_of_work: U, consent: C, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            consent,
            admin_reader,
            admin_only: true,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, A> DeleteUserUseCase for DeleteUserHandler<U, C, A>
where
    U: UnitOfWork,
    C: MarketingConsentIntentsFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx> + UserAdminMutationGuardFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "delete_user",
        skip_all,
        fields(
            user_id = %command.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteUserCommand,
    ) -> Result<DeleteUserResult, DeleteUserError> {
        context
            .require()
            .credential_capability(CredentialCapability::UsersWrite)
            .authorize::<DeleteUserError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| DeleteUserError::BeginTransactionFailed)?;
        authorize_delete_user(
            context,
            command.user_id,
            self.admin_only,
            &mut tx,
            &self.admin_reader,
        )
        .await?;
        match UserAdminMutationGuardFactory::in_transaction(&self.admin_reader, &mut tx)
            .check_removal(command.user_id)
            .await?
        {
            UserAdminRemovalDecision::TargetNotFound => {
                return Err(DeleteUserError::UserNotFound);
            }
            UserAdminRemovalDecision::LastAdmin => {
                return Err(DeleteUserError::LastAdminProtected);
            }
            UserAdminRemovalDecision::TargetNotAdmin | UserAdminRemovalDecision::Allowed => {}
        }
        // The coordinator owns the stable key and the atomic deletion/revoke decision.
        let consent = MarketingConsentCoordinator::new(&mut tx, &self.consent)
            .user_deletion_with_evidence(command.user_id, OffsetDateTime::now_utc())
            .await?;

        tx.commit()
            .await
            .map_err(|_| DeleteUserError::CommitTransactionFailed)?;
        if let Some(evidence) = consent.evidence {
            evidence.emit_after_commit(Some(context));
        }

        tracing::info!(
            event = "user.deleted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %command.user_id,
            outcome = "success",
        );

        Ok(DeleteUserResult {
            user_id: command.user_id,
        })
    }
}

async fn authorize_delete_user<Tx, A>(
    context: &OperationContext,
    user_id: UserId,
    admin_only: bool,
    tx: &mut Tx,
    admin_reader: &A,
) -> Result<(), DeleteUserError>
where
    Tx: Transaction,
    A: UserAdminReaderFactory<Tx>,
{
    if admin_only {
        let mut reader = admin_reader.in_transaction(tx);
        return require_admin_actor(context, &mut reader)
            .await
            .map_err(DeleteUserError::from);
    }

    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(actor_id)
        | Principal::DelegatedUser {
            user_id: actor_id, ..
        } if *actor_id == user_id => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            let mut reader = admin_reader.in_transaction(tx);
            require_admin_actor(context, &mut reader)
                .await
                .map_err(DeleteUserError::from)
        }
        Principal::Anonymous => Err(DeleteUserError::AuthenticatedActorRequired),
    }
}

impl From<OperationAuthorizationError> for DeleteUserError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<RequireAdminActorError> for DeleteUserError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for DeleteUserError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<CoordinateMarketingConsentError> for DeleteUserError {
    fn from(error: CoordinateMarketingConsentError) -> Self {
        match error {
            CoordinateMarketingConsentError::UserNotFound => Self::UserNotFound,
            CoordinateMarketingConsentError::ConcurrencyConflict
            | CoordinateMarketingConsentError::SourceKeyConflict => Self::ConcurrencyConflict,
            CoordinateMarketingConsentError::TemporarilyUnavailable => {
                Self::TemporarilyUnavailable {
                    source: application::error::static_error("consent persistence unavailable"),
                }
            }
            CoordinateMarketingConsentError::InvalidPersistedState => Self::InvalidPersistedState {
                source: application::error::static_error("invalid persisted consent state"),
            },
            _ => Self::Internal {
                source: application::error::static_error("invalid user deletion consent decision"),
            },
        }
    }
}

impl From<MarketingConsentIntentError> for DeleteUserError {
    fn from(error: MarketingConsentIntentError) -> Self {
        match error {
            MarketingConsentIntentError::ConcurrencyConflict
            | MarketingConsentIntentError::SourceKeyConflict => Self::ConcurrencyConflict,
            MarketingConsentIntentError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            MarketingConsentIntentError::InvalidPersistedState => Self::InvalidPersistedState {
                source: application::error::static_error("invalid persisted consent state"),
            },
            MarketingConsentIntentError::InvalidInput
            | MarketingConsentIntentError::RegisteredEmail => Self::Internal {
                source: application::error::static_error("invalid user deletion consent input"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ConsentIntent, ConsentIntentSource, ConsentSubject, ConsentUser, MarketingConsentIntents,
        MarketingConsentIntentsFactory, UserAdminActorView, UserAdminMutationGuard,
        UserAdminMutationGuardFactory, UserAdminReader, UserAdminReaderFactory,
        UserAdminRemovalDecision, UserDetailsView, UserStorageVersion,
    };
    use application::error::box_error;
    use application::operation_context::{CorrelationId, RequestId};
    use application::transaction::{Transaction, TransactionError};

    use serde_email::Email;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;

    use user_core::tier::UserTier;

    #[derive(Default)]
    struct TxState {
        begin_error: bool,
        commit_error: bool,
        begins: usize,
        commits: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<TxState>>,
    }

    struct FakeTx {
        state: Arc<Mutex<TxState>>,
    }

    #[derive(Default)]
    struct RepoState {
        user: Option<ConsentUser>,
        error: Option<MarketingConsentIntentError>,
        record_error: Option<MarketingConsentIntentError>,
        find_by_id_calls: usize,
        delete_calls: usize,
        source_keys: Vec<String>,
        deleted_emails: Vec<Email>,
        intent: Option<ConsentIntent>,
    }

    #[derive(Clone, Default)]
    struct FakeConsentFactory {
        state: Arc<Mutex<RepoState>>,
    }

    struct FakeConsent {
        state: Arc<Mutex<RepoState>>,
    }

    #[derive(Clone, Default)]
    struct FakeUserAdminReaderFactory {
        state: Arc<Mutex<AdminReadState>>,
    }

    #[derive(Default)]
    struct AdminReadState {
        user: Option<UserDetailsView>,
        calls: usize,
        removal_decision: UserAdminRemovalDecision,
    }

    struct FakeUserAdminReader {
        state: Arc<Mutex<AdminReadState>>,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn ctx(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn consent_user(id: UserId) -> ConsentUser {
        ConsentUser {
            user_id: id,
            email: email("actor@example.com"),
            version: UserStorageVersion::INITIAL,
            marketing_email_consent: false,
            marketing_email_consent_revision: 0,
            marketing_email_consent_changed_at: None,
        }
    }

    fn available_user(factory: &FakeConsentFactory, id: UserId) {
        lock(&factory.state).user = Some(consent_user(id));
    }

    fn user_details(user_id: UserId, role: UserRole) -> UserDetailsView {
        UserDetailsView {
            user_id,
            email: email("actor@example.com"),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            measurement_unit: None,
            show_unassessed_or_sensitive_content: false,
            marketing_email_consent: false,
            tier: UserTier::Free,
            role,
            stripe_customer_id: None,
        }
    }

    fn admin_reader(user_id: UserId, role: UserRole) -> FakeUserAdminReaderFactory {
        let reader = FakeUserAdminReaderFactory::default();
        lock(&reader.state).user = Some(user_details(user_id, role));
        reader
    }

    fn no_admin_reader() -> FakeUserAdminReaderFactory {
        FakeUserAdminReaderFactory::default()
    }

    fn assert_error<T, F>(result: Result<T, DeleteUserError>, predicate: F)
    where
        F: FnOnce(&DeleteUserError) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            state.commits += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.state);
            state.begins += 1;
            if state.begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl MarketingConsentIntents for FakeConsent {
        async fn lock_recipient(&mut self, _: &Email) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }

        async fn lock_source_key(&mut self, _: &str) -> Result<(), MarketingConsentIntentError> {
            unreachable!()
        }

        async fn find_by_source_key(
            &mut self,
            key: &str,
        ) -> Result<Option<ConsentIntent>, MarketingConsentIntentError> {
            Ok(lock(&self.state)
                .intent
                .as_ref()
                .filter(|intent| intent.source_key == key)
                .cloned())
        }
        async fn find_user_by_id(
            &mut self,
            id: UserId,
        ) -> Result<Option<ConsentUser>, MarketingConsentIntentError> {
            let mut state = lock(&self.state);
            state.find_by_id_calls += 1;
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            Ok(state
                .user
                .as_ref()
                .filter(|user| user.user_id == id)
                .cloned())
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
            _: Option<crate::ports::NewsletterProfile>,
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
            _: Option<crate::ports::NewsletterProfile>,
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
            _: user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<crate::ports::GrantRaceRepairOutcome, MarketingConsentIntentError> {
            unreachable!()
        }

        async fn record_user_deletion(
            &mut self,
            user: &ConsentUser,
            source_key: &str,
            _: OffsetDateTime,
        ) -> Result<ConsentIntent, MarketingConsentIntentError> {
            let mut state = lock(&self.state);
            state.delete_calls += 1;
            if let Some(error) = state.record_error.take() {
                return Err(error);
            }
            state.source_keys.push(source_key.to_owned());
            state.deleted_emails.push(user.email.clone());
            let intent = ConsentIntent {
                intent_id:
                    user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId::new(),
                source_key: source_key.to_owned(),
                subject: ConsentSubject::User(user.user_id),
                source: ConsentIntentSource::UserDeletion,
                email: user.email.clone(),
                profile_snapshot: None,

                desired: false,
            };
            state.intent = Some(intent.clone());
            Ok(intent)
        }
    }

    impl MarketingConsentIntentsFactory<FakeTx> for FakeConsentFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl MarketingConsentIntents + 'tx {
            FakeConsent {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeUserAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            Ok(state.user.clone().map(|user| UserAdminActorView {
                user_id: user.user_id,
                role: user.role,
            }))
        }
    }

    #[async_trait::async_trait]
    impl UserAdminMutationGuard for FakeUserAdminReader {
        async fn check_removal(
            &mut self,
            _user_id: UserId,
        ) -> Result<UserAdminRemovalDecision, UserAdminReadError> {
            Ok(lock(&self.state).removal_decision)
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeUserAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeUserAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl UserAdminMutationGuardFactory<FakeTx> for FakeUserAdminReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl UserAdminMutationGuard + 'tx {
            FakeUserAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_delete_own_user_without_admin_lookup() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeConsentFactory::default();
        available_user(&users, user_id);
        let handler =
            DeleteUserHandler::new(unit_of_work.clone(), users.clone(), no_admin_reader());

        let result = handler
            .execute(
                &ctx(Principal::User(user_id)),
                DeleteUserCommand { user_id },
            )
            .await;

        match result {
            Ok(result) => assert_eq!(user_id, result.user_id),
            Err(error) => panic!("delete failed: {error:?}"),
        }
        let state = lock(&users.state);
        assert_eq!(1, state.find_by_id_calls);
        assert_eq!(1, state.delete_calls);
        assert_eq!(
            state.source_keys,
            [
                crate::use_cases::commands::coordinate_marketing_consent::user_deletion_source_key(
                    user_id
                )
            ]
        );
        assert_eq!(state.deleted_emails, [email("actor@example.com")]);
        assert_eq!(1, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_use_same_source_key_for_system_and_authenticated_deletion() {
        let user_id = UserId::new();
        let system_consent = FakeConsentFactory::default();
        let self_consent = FakeConsentFactory::default();
        available_user(&system_consent, user_id);
        available_user(&self_consent, user_id);
        for (principal, consent) in [
            (Principal::System, system_consent.clone()),
            (Principal::User(user_id), self_consent.clone()),
        ] {
            DeleteUserHandler::new(FakeUnitOfWork::default(), consent, no_admin_reader())
                .execute(&ctx(principal), DeleteUserCommand { user_id })
                .await
                .unwrap();
        }
        assert_eq!(
            lock(&system_consent.state).source_keys,
            lock(&self_consent.state).source_keys
        );
    }

    #[tokio::test]
    async fn should_not_commit_when_consent_deletion_conflicts() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let consent = FakeConsentFactory::default();
        available_user(&consent, user_id);
        lock(&consent.state).record_error = Some(MarketingConsentIntentError::ConcurrencyConflict);
        let handler =
            DeleteUserHandler::new(unit_of_work.clone(), consent.clone(), no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::ConcurrencyConflict),
        );
        assert_eq!(1, lock(&consent.state).delete_calls);
        assert_eq!(0, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_reject_conflicting_deletion_replay_without_commit() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let consent = FakeConsentFactory::default();
        available_user(&consent, user_id);
        let mut conflicting = ConsentIntent {
            intent_id:
                user_core::marketing_consent_sync_intent_id::MarketingConsentSyncIntentId::new(),
            source_key:
                crate::use_cases::commands::coordinate_marketing_consent::user_deletion_source_key(
                    user_id,
                ),
            subject: ConsentSubject::User(user_id),
            source: ConsentIntentSource::UserDeletion,
            email: email("actor@example.com"),
            profile_snapshot: None,
            desired: true,
        };
        lock(&consent.state).intent = Some(conflicting.clone());
        let handler =
            DeleteUserHandler::new(unit_of_work.clone(), consent.clone(), no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::ConcurrencyConflict),
        );
        assert_eq!(lock(&unit_of_work.state).commits, 0);
        assert_eq!(lock(&consent.state).delete_calls, 0);
        conflicting.desired = false;
        conflicting.subject = ConsentSubject::EmailOnly;
        lock(&consent.state).intent = Some(conflicting);
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::ConcurrencyConflict),
        );
        assert_eq!(lock(&unit_of_work.state).commits, 0);
    }

    #[tokio::test]
    async fn should_allow_admin_to_delete_other_user() {
        let admin_id = UserId::new();
        let target_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeConsentFactory::default();
        available_user(&users, target_id);
        let admin_reader = admin_reader(admin_id, UserRole::Admin);
        let handler =
            DeleteUserHandler::new_admin_only(unit_of_work, users.clone(), admin_reader.clone());

        let result = handler
            .execute(
                &ctx(Principal::User(admin_id)),
                DeleteUserCommand { user_id: target_id },
            )
            .await;

        match result {
            Ok(result) => assert_eq!(target_id, result.user_id),
            Err(error) => panic!("delete failed: {error:?}"),
        }
        assert_eq!(1, lock(&admin_reader.state).calls);
        assert_eq!(1, lock(&users.state).find_by_id_calls);
        assert_eq!(1, lock(&users.state).delete_calls);
    }

    #[tokio::test]
    async fn should_reject_non_admin_self_delete_through_admin_handler() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeConsentFactory::default();
        available_user(&users, user_id);
        let admin_reader = admin_reader(user_id, UserRole::User);
        let handler =
            DeleteUserHandler::new_admin_only(unit_of_work.clone(), users.clone(), admin_reader);

        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::Forbidden),
        );
        assert_eq!(0, lock(&users.state).delete_calls);
        assert_eq!(0, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_reject_anonymous_non_admin_and_delegated_without_scope() {
        let actor_id = UserId::new();
        let target_id = UserId::new();
        let handler = DeleteUserHandler::new(
            FakeUnitOfWork::default(),
            FakeConsentFactory::default(),
            no_admin_reader(),
        );

        assert_error(
            handler
                .execute(
                    &ctx(Principal::Anonymous),
                    DeleteUserCommand { user_id: target_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::AuthenticatedActorRequired),
        );
        assert_error(
            handler
                .execute(
                    &ctx(Principal::DelegatedUser {
                        user_id: actor_id,
                        capabilities: BTreeSet::new(),
                    }),
                    DeleteUserCommand { user_id: actor_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::Forbidden),
        );

        let users = FakeConsentFactory::default();
        available_user(&users, target_id);
        let handler = DeleteUserHandler::new(
            FakeUnitOfWork::default(),
            users,
            admin_reader(actor_id, UserRole::User),
        );
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(actor_id)),
                    DeleteUserCommand { user_id: target_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::Forbidden),
        );
    }

    #[tokio::test]
    async fn should_protect_last_admin_deletion_without_delete_or_commit() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeConsentFactory::default();
        available_user(&users, user_id);
        let admin_reader = admin_reader(user_id, UserRole::Admin);
        lock(&admin_reader.state).removal_decision = UserAdminRemovalDecision::LastAdmin;

        let result = DeleteUserHandler::new(unit_of_work.clone(), users.clone(), admin_reader)
            .execute(&ctx(Principal::System), DeleteUserCommand { user_id })
            .await;

        assert_error(result, |error| {
            matches!(error, DeleteUserError::LastAdminProtected)
        });
        assert_eq!(0, lock(&users.state).delete_calls);
        assert_eq!(0, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_map_not_found_repo_begin_and_commit_errors() {
        let user_id = UserId::new();
        let users = FakeConsentFactory::default();
        let handler =
            DeleteUserHandler::new(FakeUnitOfWork::default(), users.clone(), no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::UserNotFound),
        );

        available_user(&users, user_id);
        lock(&users.state).error = Some(MarketingConsentIntentError::TemporarilyUnavailable {
            source: box_error(std::io::Error::other("unavailable")),
        });
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::TemporarilyUnavailable { .. }),
        );

        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        let handler =
            DeleteUserHandler::new(begin_uow, FakeConsentFactory::default(), no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::BeginTransactionFailed),
        );

        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let users = FakeConsentFactory::default();
        available_user(&users, user_id);
        let handler = DeleteUserHandler::new(commit_uow, users, no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::CommitTransactionFailed),
        );
    }
}
