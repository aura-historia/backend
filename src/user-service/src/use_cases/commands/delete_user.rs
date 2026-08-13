use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserRepository, UserRepositoryError,
    UserRepositoryFactory,
};
use crate::use_cases::authorization::{RequireAdminActorError, require_admin_actor};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;

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

pub struct DeleteUserHandler<U, R, A> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
}

impl<U, R, A> DeleteUserHandler<U, R, A> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> DeleteUserUseCase for DeleteUserHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
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
        authorize_delete_user(context, command.user_id, &mut tx, &self.admin_reader).await?;
        let mut users = self.users.in_transaction(&mut tx);
        let deleted = users.delete_by_id(command.user_id).await?;
        drop(users);

        if !deleted {
            return Err(DeleteUserError::UserNotFound);
        }

        tx.commit()
            .await
            .map_err(|_| DeleteUserError::CommitTransactionFailed)?;

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
    tx: &mut Tx,
    admin_reader: &A,
) -> Result<(), DeleteUserError>
where
    Tx: Transaction,
    A: UserAdminReaderFactory<Tx>,
{
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

impl From<UserRepositoryError> for DeleteUserError {
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
    use super::*;
    use crate::ports::{
        UserAdminActorView, UserAdminReader, UserAdminReaderFactory, UserDetailsView,
        UserStorageVersion, VersionedUser,
    };
    use common::error::boxed::{BoxError, box_error};
    use common::operation_context::{CorrelationId, RequestId};
    use common::stripe_customer_id::StripeCustomerId;
    use common::transaction::{Transaction, TransactionError};
    use common::versioned::Versioned;
    use serde_email::Email;
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
    use user_core::tier::UserTier;
    use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};

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
        user: Option<User>,
        delete_result: bool,
        error: Option<UserRepositoryError>,
        find_by_id_calls: usize,
        delete_calls: usize,
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
        calls: usize,
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

    fn user_with(id: UserId, role: UserRole) -> User {
        match User::create(NewUser {
            id,
            email: email("actor@example.com"),
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier: UserTier::Free,
                role,
                stripe_customer_id: None,
            },
        }) {
            Ok(user) => user,
            Err(error) => panic!("invalid test user: {error}"),
        }
    }

    fn boxed() -> BoxError {
        box_error(std::io::Error::other("boom"))
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
            prohibited_content_consent: false,
            tier: UserTier::Free,
            role,
            stripe_customer_id: None,
            structured_address: None,
            geo_address: None,
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
    impl UserRepository for FakeUserRepository {
        async fn find_by_id(
            &mut self,
            _id: UserId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.find_by_id_calls += 1;
            if let Some(error) = state.error.take() {
                Err(error)
            } else {
                Ok(state
                    .user
                    .clone()
                    .map(|value| Versioned::new(value, UserStorageVersion::INITIAL)))
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
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn insert_if_absent(
            &mut self,
            user: &User,
        ) -> Result<crate::ports::UserInsertOutcome, UserRepositoryError> {
            Ok(crate::ports::UserInsertOutcome::Created(Versioned::new(
                user.clone(),
                UserStorageVersion::INITIAL,
            )))
        }

        async fn update(
            &mut self,
            user: &User,
            _expected_version: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            Ok(Versioned::new(user.clone(), UserStorageVersion::INITIAL))
        }

        async fn delete_by_id(&mut self, _id: UserId) -> Result<bool, UserRepositoryError> {
            let mut state = lock(&self.state);
            state.delete_calls += 1;
            if let Some(error) = state.error.take() {
                Err(error)
            } else {
                Ok(state.delete_result)
            }
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
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            let mut state = lock(&self.state);
            state.calls += 1;
            Ok(state.user.clone().map(|user| UserAdminActorView {
                user_id: user.user_id,
                role: user.role,
            }))
        }
    }

    impl UserAdminReaderFactory<FakeTx> for FakeUserAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeUserAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_delete_own_user_without_admin_lookup() {
        let user_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        lock(&users.state).delete_result = true;
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
        assert_eq!(0, lock(&users.state).find_by_id_calls);
        assert_eq!(1, lock(&users.state).delete_calls);
        assert_eq!(1, lock(&unit_of_work.state).commits);
    }

    #[tokio::test]
    async fn should_allow_admin_to_delete_other_user() {
        let admin_id = UserId::new();
        let target_id = UserId::new();
        let unit_of_work = FakeUnitOfWork::default();
        let users = FakeUserRepositoryFactory::default();
        {
            let mut state = lock(&users.state);
            state.user = Some(user_with(admin_id, UserRole::Admin));
            state.delete_result = true;
        }
        let admin_reader = admin_reader(admin_id, UserRole::Admin);
        let handler = DeleteUserHandler::new(unit_of_work, users.clone(), admin_reader.clone());

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
        assert_eq!(0, lock(&users.state).find_by_id_calls);
        assert_eq!(1, lock(&users.state).delete_calls);
    }

    #[tokio::test]
    async fn should_reject_anonymous_non_admin_and_delegated_without_scope() {
        let actor_id = UserId::new();
        let target_id = UserId::new();
        let handler = DeleteUserHandler::new(
            FakeUnitOfWork::default(),
            FakeUserRepositoryFactory::default(),
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

        let users = FakeUserRepositoryFactory::default();
        {
            let mut state = lock(&users.state);
            state.user = Some(user_with(actor_id, UserRole::User));
            state.delete_result = true;
        }
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
    async fn should_map_not_found_repo_begin_and_commit_errors() {
        let user_id = UserId::new();
        let users = FakeUserRepositoryFactory::default();
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

        lock(&users.state).delete_result = true;
        lock(&users.state).error = Some(UserRepositoryError::Internal { source: boxed() });
        assert_error(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    DeleteUserCommand { user_id },
                )
                .await,
            |error| matches!(error, DeleteUserError::Internal { .. }),
        );

        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        let handler = DeleteUserHandler::new(
            begin_uow,
            FakeUserRepositoryFactory::default(),
            no_admin_reader(),
        );
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
        let users = FakeUserRepositoryFactory::default();
        lock(&users.state).delete_result = true;
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
