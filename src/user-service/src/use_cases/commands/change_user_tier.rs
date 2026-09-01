use crate::ports::{
    UserAdminReadError, UserAdminReaderFactory, UserDetailsView, UserRepository,
    UserRepositoryError, UserRepositoryFactory, UserTierEntitlements, UserTierEntitlementsError,
    UserTierEntitlementsFactory,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::error::BoxError;
use application::operation_context::{CredentialCapability, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use user_core::user_id::UserId;
use user_core::{tier::UserTier, user::User};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierCommand {
    pub user_id: UserId,
    pub tier: UserTier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserTierResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserTierError {
    #[error("authenticated actor required to change user tier")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user tier entitlement lock failed")]
    TierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("user tier entitlement reconciliation failed")]
    TierEntitlementsReconciliationFailed {
        #[source]
        source: BoxError,
    },
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
    #[error("failed to begin change user tier transaction")]
    BeginTransactionFailed,
    #[error("failed to commit change user tier transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ChangeUserTierUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeUserTierCommand,
    ) -> Result<ChangeUserTierResult, ChangeUserTierError>;
}

pub struct ChangeUserTierHandler<U, R, A, E> {
    unit_of_work: U,
    users: R,
    admin_reader: A,
    tier_entitlements: E,
}

impl<U, R, A, E> ChangeUserTierHandler<U, R, A, E> {
    pub fn new(unit_of_work: U, users: R, admin_reader: A, tier_entitlements: E) -> Self {
        Self {
            unit_of_work,
            users,
            admin_reader,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A, E> ChangeUserTierUseCase for ChangeUserTierHandler<U, R, A, E>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
    E: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "change_user_tier",
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
        command: ChangeUserTierCommand,
    ) -> Result<ChangeUserTierResult, ChangeUserTierError> {
        require_admin_actor_credential(context, CredentialCapability::UsersWrite)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeUserTierError::BeginTransactionFailed)?;
        {
            let mut admin_reader = self.admin_reader.in_transaction(&mut tx);
            require_admin_actor(context, &mut admin_reader).await?;
        }
        self.tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(command.user_id)
            .await?
            .ok_or(ChangeUserTierError::UserNotFound)?;

        let domain_primitives::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(ChangeUserTierError::UserNotFound)?;

        let outcome = user.change_tier(command.tier);
        if outcome.changed() {
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
            self.tier_entitlements
                .in_transaction(&mut tx)
                .reconcile_for_tier(command.user_id, user.account().tier)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| ChangeUserTierError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.tier_changed",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            tier = ?user.account().tier,
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(ChangeUserTierResult::from(&user))
    }
}

impl From<&User> for ChangeUserTierResult {
    fn from(user: &User) -> Self {
        Self {
            view: UserDetailsView::from(user),
        }
    }
}

impl From<RequireAdminActorError> for ChangeUserTierError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for ChangeUserTierError {
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

impl From<UserTierEntitlementsError> for ChangeUserTierError {
    fn from(error: UserTierEntitlementsError) -> Self {
        match error {
            UserTierEntitlementsError::LockFailed { source } => {
                Self::TierEntitlementsLockFailed { source }
            }
            UserTierEntitlementsError::ReconciliationFailed { source } => {
                Self::TierEntitlementsReconciliationFailed { source }
            }
        }
    }
}

impl From<UserRepositoryError> for ChangeUserTierError {
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
        ChangeUserTierCommand, ChangeUserTierError, ChangeUserTierHandler, ChangeUserTierUseCase,
    };
    use user_core::user_id::UserId;

    use crate::ports::{
        UserAdminActorView, UserAdminReader, UserAdminReaderFactory, UserDetailsView,
        UserRepository, UserRepositoryError, UserRepositoryFactory, UserStorageVersion,
        UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
        VersionedUser,
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
        lock_user_tier_calls: usize,
        reconcile_for_tier_calls: usize,
    }

    #[derive(Clone, Default)]
    struct FakeUserRepositoryFactory {
        state: Arc<Mutex<RepoState>>,
    }

    struct FakeUserRepository {
        state: Arc<Mutex<RepoState>>,
    }

    struct FakeUserTierEntitlements {
        state: Arc<Mutex<RepoState>>,
    }

    #[derive(Clone, Default)]
    struct FakeUserAdminReaderFactory {
        state: Arc<Mutex<AdminReadState>>,
    }

    #[derive(Default)]
    struct AdminReadState {
        user: Option<UserDetailsView>,
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

    impl UserTierEntitlementsFactory<FakeTx> for FakeUserRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserTierEntitlements + 'tx {
            FakeUserTierEntitlements {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserTierEntitlements for FakeUserTierEntitlements {
        async fn lock_user_tier(
            &mut self,
            _user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            let mut state = lock(&self.state);
            state.lock_user_tier_calls += 1;
            Ok(state.user.as_ref().map(|user| user.value.account().tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            lock(&self.state).reconcile_for_tier_calls += 1;
            Ok(())
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

    impl UserAdminReaderFactory<FakeTx> for FakeUserAdminReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserAdminReader + 'tx {
            FakeUserAdminReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_change_user_tier() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        let begin_repo = FakeUserRepositoryFactory::default();
        assert_error(
            ChangeUserTierHandler::new(
                begin_uow,
                begin_repo.clone(),
                no_admin_reader(),
                begin_repo,
            )
            .execute(
                &ctx(Principal::System),
                ChangeUserTierCommand {
                    user_id,
                    tier: UserTier::Pro,
                },
            )
            .await,
            |error| matches!(error, ChangeUserTierError::BeginTransactionFailed),
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
            ChangeUserTierHandler::new(commit_uow, repo.clone(), no_admin_reader(), repo)
                .execute(
                    &ctx(Principal::System),
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Pro,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserTierError::CommitTransactionFailed),
        );
    }

    #[tokio::test]
    async fn should_allow_admin_user_and_reject_non_admin_user_for_change_user_tier() {
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
            ChangeUserTierHandler::new(
                uow.clone(),
                repo.clone(),
                admin_reader(user_id, UserRole::Admin),
                repo.clone(),
            )
            .execute(
                &ctx(Principal::User(user_id)),
                ChangeUserTierCommand {
                    user_id,
                    tier: UserTier::Pro,
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
            ChangeUserTierHandler::new(
                uow,
                repo.clone(),
                admin_reader(user_id, UserRole::User),
                repo.clone(),
            )
            .execute(
                &ctx(Principal::User(user_id)),
                ChangeUserTierCommand {
                    user_id,
                    tier: UserTier::Ultimate,
                },
            )
            .await,
            |error| matches!(error, ChangeUserTierError::Forbidden),
        );
    }

    #[tokio::test]
    async fn should_reject_delegated_user_without_users_write_before_tx_for_change_user_tier() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();

        assert_error(
            ChangeUserTierHandler::new(uow.clone(), repo.clone(), no_admin_reader(), repo)
                .execute(
                    &ctx(Principal::DelegatedUser {
                        user_id,
                        capabilities: BTreeSet::new(),
                    }),
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Pro,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserTierError::Forbidden),
        );
        assert_eq!(0, lock(&uow.state).begins);
    }

    #[tokio::test]
    async fn should_change_tier_success_not_found_noop_and_repo_error() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        let handler =
            ChangeUserTierHandler::new(uow.clone(), repo.clone(), no_admin_reader(), repo.clone());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Pro,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserTierError::UserNotFound),
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
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Pro,
                    },
                )
                .await,
        );
        assert_eq!(1, lock(&repo.state).update_calls);
        assert_ok(
            handler
                .execute(
                    &ctx(Principal::System),
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Pro,
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
                    ChangeUserTierCommand {
                        user_id,
                        tier: UserTier::Ultimate,
                    },
                )
                .await,
            |error| matches!(error, ChangeUserTierError::Internal { .. }),
        );
        assert_eq!(4, lock(&repo.state).lock_user_tier_calls);
        assert_eq!(1, lock(&repo.state).reconcile_for_tier_calls);
    }
}
