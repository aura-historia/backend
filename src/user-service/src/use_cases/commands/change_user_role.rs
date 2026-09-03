use crate::ports::{
    UserAdminMutationGuard, UserAdminMutationGuardFactory, UserAdminReadError,
    UserAdminReaderFactory, UserAdminRemovalDecision, UserDetailsView, UserRepository,
    UserRepositoryError, UserRepositoryFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;
use user_core::{role::UserRole, user::User};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleCommand {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserRoleError {
    #[error("authenticated actor required to change user role")]
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
    #[error("failed to begin change user role transaction")]
    BeginTransactionFailed,
    #[error("failed to commit change user role transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ChangeUserRoleUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeUserRoleCommand,
    ) -> Result<ChangeUserRoleResult, ChangeUserRoleError>;
}

pub struct ChangeUserRoleHandler<U, R, A> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
}

impl<U, R, A> ChangeUserRoleHandler<U, R, A> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> ChangeUserRoleUseCase for ChangeUserRoleHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx> + UserAdminMutationGuardFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "change_user_role",
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
        command: ChangeUserRoleCommand,
    ) -> Result<ChangeUserRoleResult, ChangeUserRoleError> {
        require_admin_actor_credential(context, CredentialCapability::UsersWrite)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeUserRoleError::BeginTransactionFailed)?;
        {
            let mut admin_reader =
                UserAdminReaderFactory::in_transaction(&self.admin_reader, &mut tx);
            require_admin_actor(context, &mut admin_reader).await?;
        }
        let domain_primitives::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(ChangeUserRoleError::UserNotFound)?;

        let previous_role = user.account().role;
        let outcome = user.change_role(command.role);
        if outcome.changed() {
            match UserAdminMutationGuardFactory::in_transaction(&self.admin_reader, &mut tx)
                .check_removal(command.user_id)
                .await?
            {
                UserAdminRemovalDecision::TargetNotFound => {
                    return Err(ChangeUserRoleError::UserNotFound);
                }
                UserAdminRemovalDecision::TargetNotAdmin
                    if previous_role == UserRole::Admin && command.role != UserRole::Admin =>
                {
                    return Err(ChangeUserRoleError::ConcurrencyConflict);
                }
                UserAdminRemovalDecision::LastAdmin
                    if previous_role == UserRole::Admin && command.role != UserRole::Admin =>
                {
                    return Err(ChangeUserRoleError::LastAdminProtected);
                }
                UserAdminRemovalDecision::TargetNotAdmin
                | UserAdminRemovalDecision::Allowed
                | UserAdminRemovalDecision::LastAdmin => {}
            }
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
        }

        tx.commit()
            .await
            .map_err(|_| ChangeUserRoleError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.role_changed",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            role = ?user.account().role,
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(ChangeUserRoleResult::from(&user))
    }
}

impl From<&User> for ChangeUserRoleResult {
    fn from(user: &User) -> Self {
        Self {
            view: UserDetailsView::from(user),
        }
    }
}

impl From<RequireAdminActorError> for ChangeUserRoleError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for ChangeUserRoleError {
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

impl From<UserRepositoryError> for ChangeUserRoleError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source } => Self::EmailConflict { source },
            UserRepositoryError::StripeCustomerConflict { source } => {
                Self::StripeCustomerConflict { source }
            }
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            UserRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(dead_code, unused_imports)]
    use super::{
        ChangeUserRoleCommand, ChangeUserRoleError, ChangeUserRoleHandler, ChangeUserRoleUseCase,
    };
    use user_core::user_id::UserId;

    use crate::ports::{
        UserAdminActorView, UserAdminMutationGuard, UserAdminMutationGuardFactory, UserAdminReader,
        UserAdminReaderFactory, UserAdminRemovalDecision, UserDetailsView, UserRepository,
        UserRepositoryError, UserRepositoryFactory, UserStorageVersion, VersionedUser,
    };
    use application::error::{BoxError, box_error};
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use domain_primitives::versioned::Versioned;
    use serde_email::Email;
    use std::collections::BTreeSet;
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
    use user_core::stripe_customer_id::StripeCustomerId;
    use user_core::tier::UserTier;
    use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};

    #[derive(Debug, Clone, Copy)]
    enum RepoErrorKind {
        ConcurrencyConflict,
        EmailConflict,
        StripeCustomerConflict,
        TemporarilyUnavailable,
        InvalidPersistedState,
        Internal,
    }

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
        user: Option<VersionedUser>,
        find_by_id_error: Option<RepoErrorKind>,
        insert_error: Option<RepoErrorKind>,
        update_error: Option<RepoErrorKind>,
        find_by_id_calls: usize,
        insert_calls: usize,
        update_calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUserRepositoryFactory {
        state: Arc<Mutex<RepoState>>,
    }

    struct FakeUserRepository {
        state: Arc<Mutex<RepoState>>,
    }

    #[derive(Clone, Default)]
    struct FakeUserAdminReaderFactory {
        state: Arc<Mutex<AdminReadState>>,
    }

    #[derive(Default)]
    struct AdminReadState {
        user: Option<UserDetailsView>,
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

    fn email(value: &str) -> Email {
        match Email::try_from(value) {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }

    fn ctx(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn user_with(
        id: user_core::user_id::UserId,
        email_value: &str,
        role: UserRole,
        tier: UserTier,
    ) -> User {
        match User::create(NewUser {
            id,
            email: email(email_value),
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier,
                role,
                stripe_customer_id: None,
            },
        }) {
            Ok(user) => user,
            Err(error) => panic!("invalid test user: {error}"),
        }
    }

    fn versioned(user: User) -> VersionedUser {
        Versioned {
            value: user,
            version: UserStorageVersion::INITIAL,
        }
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

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
    }

    fn repo_error(kind: RepoErrorKind) -> UserRepositoryError {
        match kind {
            RepoErrorKind::ConcurrencyConflict => UserRepositoryError::ConcurrencyConflict,
            RepoErrorKind::EmailConflict => UserRepositoryError::EmailConflict { source: boxed() },
            RepoErrorKind::StripeCustomerConflict => {
                UserRepositoryError::StripeCustomerConflict { source: boxed() }
            }
            RepoErrorKind::TemporarilyUnavailable => {
                UserRepositoryError::TemporarilyUnavailable { source: boxed() }
            }
            RepoErrorKind::InvalidPersistedState => {
                UserRepositoryError::InvalidPersistedState { source: boxed() }
            }
            RepoErrorKind::Internal => UserRepositoryError::Internal { source: boxed() },
        }
    }

    fn assert_error<T, E, F>(result: Result<T, E>, predicate: F)
    where
        E: Debug,
        F: FnOnce(&E) -> bool,
    {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => assert!(predicate(&error), "unexpected error: {error:?}"),
        }
    }

    fn assert_ok<T, E: Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected ok, got {error:?}"),
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                state.commits += 1;
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
    impl UserRepository for FakeUserRepository {
        async fn find_by_id(
            &mut self,
            _id: user_core::user_id::UserId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.find_by_id_calls += 1;
            if let Some(kind) = state.find_by_id_error {
                Err(repo_error(kind))
            } else {
                Ok(state.user.clone())
            }
        }

        async fn find_by_email(
            &mut self,
            _email: &Email,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn find_by_stripe_customer_id(
            &mut self,
            _stripe_customer_id: &StripeCustomerId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, user: &User) -> Result<VersionedUser, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            if let Some(kind) = state.insert_error {
                Err(repo_error(kind))
            } else {
                let user = versioned(user.clone());
                state.user = Some(user.clone());
                Ok(user)
            }
        }

        async fn insert_if_absent(
            &mut self,
            user: &User,
        ) -> Result<crate::ports::UserInsertOutcome, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            if let Some(kind) = state.insert_error {
                Err(repo_error(kind))
            } else {
                let user = versioned(user.clone());
                state.user = Some(user.clone());
                Ok(crate::ports::UserInsertOutcome::Created(user))
            }
        }

        async fn update(
            &mut self,
            user: &User,
            _expected_version: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.update_calls += 1;
            if let Some(kind) = state.update_error {
                Err(repo_error(kind))
            } else {
                let user = versioned(user.clone());
                state.user = Some(user.clone());
                Ok(user)
            }
        }

        async fn delete_by_id(&mut self, _id: UserId) -> Result<bool, UserRepositoryError> {
            Ok(true)
        }
    }

    impl UserRepositoryFactory<FakeTx> for FakeUserRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserRepository + 'tx {
            FakeUserRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeUserAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserAdminActorView>, crate::ports::UserAdminReadError> {
            Ok(lock(&self.state)
                .user
                .clone()
                .map(|user| UserAdminActorView {
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
        ) -> Result<UserAdminRemovalDecision, crate::ports::UserAdminReadError> {
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
    async fn should_map_begin_and_commit_failures_for_change_user_role() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            ChangeUserRoleHandler::new(
                begin_uow,
                FakeUserRepositoryFactory::default(),
                no_admin_reader(),
            )
            .execute(
                &ctx(Principal::System),
                ChangeUserRoleCommand {
                    user_id,
                    role: UserRole::Admin,
                },
            )
            .await,
            |error| matches!(error, ChangeUserRoleError::BeginTransactionFailed),
        );

        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let repo = FakeUserRepositoryFactory::default();
        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        )));
        assert_error(
            ChangeUserRoleHandler::new(commit_uow, repo, no_admin_reader())
                .execute(
                    &ctx(Principal::System),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserRoleError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_allow_admin_user_and_reject_non_admin_user_for_change_user_role() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "admin@example.com",
            UserRole::Admin,
            UserTier::Free,
        )));

        assert_ok(
            ChangeUserRoleHandler::new(
                uow.clone(),
                repo.clone(),
                admin_reader(user_id, UserRole::Admin),
            )
            .execute(
                &ctx(Principal::User(user_id)),
                ChangeUserRoleCommand {
                    user_id,
                    role: UserRole::User,
                },
            )
            .await,
        );

        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "user@example.com",
            UserRole::User,
            UserTier::Free,
        )));
        assert_error(
            ChangeUserRoleHandler::new(uow, repo.clone(), admin_reader(user_id, UserRole::User))
                .execute(
                    &ctx(Principal::User(user_id)),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserRoleError::Forbidden),
        );
    }

    #[tokio::test]
    async fn should_reject_delegated_user_without_users_write_before_tx_for_change_user_role() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();

        assert_error(
            ChangeUserRoleHandler::new(uow.clone(), repo, no_admin_reader())
                .execute(
                    &ctx(Principal::DelegatedUser {
                        user_id,
                        capabilities: BTreeSet::new(),
                    }),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserRoleError::Forbidden),
        );
        assert_eq!(0, lock(&uow.state).begins);
    }

    #[tokio::test]
    async fn should_protect_last_admin_role_removal_without_update_or_commit() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.state).user = Some(versioned(user_with(
            user_id,
            "last-admin@example.com",
            UserRole::Admin,
            UserTier::Free,
        )));
        let admin_reader = admin_reader(user_id, UserRole::Admin);
        lock(&admin_reader.state).removal_decision = UserAdminRemovalDecision::LastAdmin;

        let result = ChangeUserRoleHandler::new(unit_of_work.clone(), users.clone(), admin_reader)
            .execute(
                &ctx(Principal::System),
                ChangeUserRoleCommand {
                    user_id,
                    role: UserRole::User,
                },
            )
            .await;

        assert_error(result, |error| {
            matches!(error, ChangeUserRoleError::LastAdminProtected)
        });
        assert_eq!(0, lock(&users.state).update_calls);
        assert_eq!(0, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_change_role_success_not_found_noop_and_repo_error() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        let handler = ChangeUserRoleHandler::new(uow.clone(), repo.clone(), no_admin_reader());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserRoleError::UserNotFound),
        );

        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        )));
        assert_ok(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
        );
        assert_eq!(1, lock(&repo.state).update_calls);
        assert_ok(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::Admin,
                    },
                )
                .await,
        );
        assert_eq!(1, lock(&repo.state).update_calls);

        lock(&repo.state).update_error = Some(RepoErrorKind::Internal);
        assert_error(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserRoleCommand {
                        user_id,
                        role: UserRole::User,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserRoleError::Internal { .. }),
        );
    }
}
