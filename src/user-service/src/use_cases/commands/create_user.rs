use crate::ports::{UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use serde_email::Email;
use user_core::user::{
    NewUser, RehydrateUserError, User, UserAccount, UserPreferences, UserProfile,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserCommand {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserResult {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateUserError {
    #[error("authenticated actor required to create user")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user identifier already exists with a different email")]
    UserIdentityConflict,
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
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("temporary user persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user state")]
    InvalidUserState {
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
    #[error("failed to begin create user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CreateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateUserCommand,
    ) -> Result<CreateUserResult, CreateUserError>;
}

pub struct CreateUserHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> CreateUserHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CreateUserUseCase for CreateUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "create_user",
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
        command: CreateUserCommand,
    ) -> Result<CreateUserResult, CreateUserError> {
        authorize_user_write(context, command.user_id)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let user = User::try_from(command)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CreateUserError::BeginTransactionFailed)?;

        let user = match self
            .users
            .in_transaction(&mut tx)
            .insert_if_absent(&user)
            .await?
        {
            UserInsertOutcome::Created(user) => user.value,
            UserInsertOutcome::Existing(existing_user)
                if existing_user.value.email() == user.email() =>
            {
                existing_user.value
            }
            UserInsertOutcome::Existing(_) => return Err(CreateUserError::UserIdentityConflict),
        };

        tx.commit()
            .await
            .map_err(|_| CreateUserError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            outcome = "success",
        );

        Ok(CreateUserResult::from(&user))
    }
}

impl TryFrom<CreateUserCommand> for User {
    type Error = RehydrateUserError;

    fn try_from(command: CreateUserCommand) -> Result<Self, Self::Error> {
        User::create(NewUser {
            id: command.user_id,
            email: command.email,
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount::default(),
        })
    }
}

impl From<OperationAuthorizationError> for CreateUserError {
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

impl From<&User> for CreateUserResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            email: user.email().clone(),
        }
    }
}

impl From<RehydrateUserError> for CreateUserError {
    fn from(error: RehydrateUserError) -> Self {
        Self::InvalidUserState {
            source: box_error(error),
        }
    }
}

fn authorize_user_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), CreateUserError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<CreateUserError>()
}

impl From<UserRepositoryError> for CreateUserError {
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
    use super::{CreateUserCommand, CreateUserError, CreateUserHandler, CreateUserUseCase};
    use common::user_id::UserId;

    use crate::ports::{
        UserInsertOutcome, UserRepository, UserRepositoryError, UserRepositoryFactory,
        UserStorageVersion, VersionedUser,
    };
    use common::error::boxed::{BoxError, box_error};
    use common::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use common::stripe_customer_id::StripeCustomerId;
    use common::transaction::{Transaction, TransactionError, UnitOfWork};
    use common::versioned::Versioned;
    use serde_email::Email;
    use std::fmt::Debug;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
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
        insert_outcome: Option<UserInsertOutcome>,
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
        id: common::user_id::UserId,
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
            _id: common::user_id::UserId,
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
        ) -> Result<UserInsertOutcome, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.insert_calls += 1;
            if let Some(kind) = state.insert_error {
                Err(repo_error(kind))
            } else if let Some(outcome) = state.insert_outcome.clone() {
                Ok(outcome)
            } else {
                let user = versioned(user.clone());
                state.user = Some(user.clone());
                Ok(UserInsertOutcome::Created(user))
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

    #[tokio::test]
    async fn should_create_user_and_commit_when_valid() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        let handler = CreateUserHandler::new(uow.clone(), repo.clone());

        let result = assert_ok(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    CreateUserCommand {
                        user_id,
                        email: email("ada@example.com"),
                    },
                )
                .await,
        );

        assert_eq!(user_id, result.user_id);
        assert_eq!(1, lock(&uow.state).commits);
        assert_eq!(1, lock(&repo.state).insert_calls);
    }

    #[tokio::test]
    async fn should_return_existing_user_and_commit_when_create_is_replayed() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        lock(&repo.state).insert_outcome = Some(UserInsertOutcome::Existing(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        ))));
        let handler = CreateUserHandler::new(uow.clone(), repo.clone());

        let result = assert_ok(
            handler
                .execute(
                    &ctx(Principal::System),
                    CreateUserCommand {
                        user_id,
                        email: email("ada@example.com"),
                    },
                )
                .await,
        );

        assert_eq!(user_id, result.user_id);
        assert_eq!(1, lock(&uow.state).commits);
        assert_eq!(1, lock(&repo.state).insert_calls);
    }

    #[tokio::test]
    async fn should_reject_existing_user_with_different_email() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        lock(&repo.state).insert_outcome = Some(UserInsertOutcome::Existing(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        ))));
        let handler = CreateUserHandler::new(uow.clone(), repo);

        assert_error(
            handler
                .execute(
                    &ctx(Principal::System),
                    CreateUserCommand {
                        user_id,
                        email: email("grace@example.com"),
                    },
                )
                .await,
            |error| matches!(error, CreateUserError::UserIdentityConflict),
        );

        assert_eq!(0, lock(&uow.state).commits);
    }

    #[tokio::test]
    async fn should_not_begin_create_user_when_anonymous() {
        let uow = FakeUnitOfWork::default();
        let handler = CreateUserHandler::new(uow.clone(), FakeUserRepositoryFactory::default());

        assert_error(
            handler
                .execute(
                    &ctx(Principal::Anonymous),
                    CreateUserCommand {
                        user_id: UserId::new(),
                        email: email("ada@example.com"),
                    },
                )
                .await,
            |error| matches!(error, CreateUserError::AuthenticatedActorRequired),
        );

        assert_eq!(0, lock(&uow.state).begins);
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_create_user() {
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        let begin_handler = CreateUserHandler::new(begin_uow, FakeUserRepositoryFactory::default());
        assert_error(
            begin_handler
                .execute(
                    &ctx(Principal::System),
                    CreateUserCommand {
                        user_id: UserId::new(),
                        email: email("ada@example.com"),
                    },
                )
                .await,
            |error| matches!(error, CreateUserError::BeginTransactionFailed),
        );

        let commit_uow = FakeUnitOfWork::default();
        lock(&commit_uow.state).commit_error = true;
        let commit_handler =
            CreateUserHandler::new(commit_uow, FakeUserRepositoryFactory::default());
        assert_error(
            commit_handler
                .execute(
                    &ctx(Principal::System),
                    CreateUserCommand {
                        user_id: UserId::new(),
                        email: email("ada@example.com"),
                    },
                )
                .await,
            |error| matches!(error, CreateUserError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_map_create_user_repository_errors_and_not_commit() {
        for kind in [
            RepoErrorKind::ConcurrencyConflict,
            RepoErrorKind::EmailConflict,
            RepoErrorKind::StripeCustomerConflict,
            RepoErrorKind::TemporarilyUnavailable,
            RepoErrorKind::InvalidPersistedState,
            RepoErrorKind::Internal,
        ] {
            let uow = FakeUnitOfWork::default();
            let repo = FakeUserRepositoryFactory::default();
            lock(&repo.state).insert_error = Some(kind);
            let handler = CreateUserHandler::new(uow.clone(), repo);
            let result = handler
                .execute(
                    &ctx(Principal::System),
                    CreateUserCommand {
                        user_id: UserId::new(),
                        email: email("ada@example.com"),
                    },
                )
                .await;
            assert_error(result, |error| {
                matches!(
                    (kind, error),
                    (
                        RepoErrorKind::ConcurrencyConflict,
                        CreateUserError::ConcurrencyConflict
                    ) | (
                        RepoErrorKind::EmailConflict,
                        CreateUserError::EmailConflict { .. }
                    ) | (
                        RepoErrorKind::StripeCustomerConflict,
                        CreateUserError::StripeCustomerConflict { .. }
                    ) | (
                        RepoErrorKind::TemporarilyUnavailable,
                        CreateUserError::TemporarilyUnavailable { .. }
                    ) | (
                        RepoErrorKind::InvalidPersistedState,
                        CreateUserError::InvalidPersistedState { .. }
                    ) | (RepoErrorKind::Internal, CreateUserError::Internal { .. })
                )
            });
            assert_eq!(0, lock(&uow.state).commits);
        }
    }
}
