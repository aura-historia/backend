use crate::ports::{UserRepository, UserRepositoryError, UserRepositoryFactory};
use common::change_outcome::ChangeOutcome;
use common::error::boxed::{BoxError, box_error};
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::patch_field::PatchField;
use common::transaction::{Transaction, UnitOfWork};
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::StructuredAddress;
use serde_email::Email;
use user_core::user::{RehydrateUserError, User};
use user_core::{first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateUserCommand {
    pub user_id: UserId,
    pub email: PatchField<Email>,
    pub first_name: PatchField<FirstName>,
    pub last_name: PatchField<LastName>,
    pub language: PatchField<Language>,
    pub currency: PatchField<Currency>,
    pub measurement_unit: PatchField<MeasurementUnit>,
    pub prohibited_content_consent: PatchField<bool>,
    pub tier: PatchField<UserTier>,
    pub role: PatchField<UserRole>,
    pub stripe_customer_id: PatchField<StripeCustomerId>,
    pub structured_address: PatchField<StructuredAddress>,
}

impl UpdateUserCommand {
    pub fn is_empty(&self) -> bool {
        !self.email.is_changed()
            && !self.first_name.is_changed()
            && !self.last_name.is_changed()
            && !self.language.is_changed()
            && !self.currency.is_changed()
            && !self.measurement_unit.is_changed()
            && !self.prohibited_content_consent.is_changed()
            && !self.tier.is_changed()
            && !self.role.is_changed()
            && !self.stripe_customer_id.is_changed()
            && !self.structured_address.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateUserResult {
    pub user_id: UserId,
    pub email: Email,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateUserError {
    #[error("authenticated actor required to update user")]
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
    #[error("user email is required")]
    EmailRequired,
    #[error("user tier is required")]
    TierRequired,
    #[error("user role is required")]
    RoleRequired,
    #[error("invalid user state")]
    InvalidUserState {
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
    #[error("failed to begin update user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateUserCommand,
    ) -> Result<UpdateUserResult, UpdateUserError>;
}

pub struct UpdateUserHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> UpdateUserHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> UpdateUserUseCase for UpdateUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_user",
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
        command: UpdateUserCommand,
    ) -> Result<UpdateUserResult, UpdateUserError> {
        authorize_user_write(context, command.user_id)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateUserError::BeginTransactionFailed)?;
        let common::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(UpdateUserError::UserNotFound)?;

        let outcome = apply_update(&mut user, command)?;
        if outcome.changed() {
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
        }

        tx.commit()
            .await
            .map_err(|_| UpdateUserError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.updated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(UpdateUserResult::from(&user))
    }
}

impl From<OperationAuthorizationError> for UpdateUserError {
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

impl From<&User> for UpdateUserResult {
    fn from(user: &User) -> Self {
        Self {
            user_id: user.id(),
            email: user.email().clone(),
        }
    }
}

fn apply_update(
    user: &mut User,
    command: UpdateUserCommand,
) -> Result<ChangeOutcome, UpdateUserError> {
    let mut outcome = ChangeOutcome::Unchanged;

    outcome = outcome.combine(match command.email {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_email(value),
        PatchField::Clear => return Err(UpdateUserError::EmailRequired),
    });

    let mut profile = user.profile().clone();
    let profile_before = profile.clone();
    apply_optional_patch(&mut profile.first_name, command.first_name);
    apply_optional_patch(&mut profile.last_name, command.last_name);
    match command.structured_address {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            profile.structured_address = Some(value);
            profile.geo_address = None;
        }
        PatchField::Clear => {
            profile.structured_address = None;
            profile.geo_address = None;
        }
    }
    if profile != profile_before {
        outcome = outcome.combine(user.replace_profile(profile)?);
    }

    let mut preferences = user.preferences().clone();
    let preferences_before = preferences.clone();
    apply_optional_patch(&mut preferences.language, command.language);
    apply_optional_patch(&mut preferences.currency, command.currency);
    apply_optional_patch(&mut preferences.measurement_unit, command.measurement_unit);
    match command.prohibited_content_consent {
        PatchField::Unchanged => {}
        PatchField::Set(value) => preferences.prohibited_content_consent = value,
        PatchField::Clear => preferences.prohibited_content_consent = false,
    }
    if preferences != preferences_before {
        outcome = outcome.combine(user.replace_preferences(preferences));
    }

    outcome = outcome.combine(match command.tier {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_tier(value),
        PatchField::Clear => return Err(UpdateUserError::TierRequired),
    });
    outcome = outcome.combine(match command.role {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_role(value),
        PatchField::Clear => return Err(UpdateUserError::RoleRequired),
    });
    outcome = outcome.combine(match command.stripe_customer_id {
        PatchField::Unchanged => ChangeOutcome::Unchanged,
        PatchField::Set(value) => user.change_stripe_customer_id(Some(value)),
        PatchField::Clear => user.change_stripe_customer_id(None),
    });

    Ok(outcome)
}

fn apply_optional_patch<T>(target: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Unchanged => {}
        PatchField::Set(value) => *target = Some(value),
        PatchField::Clear => *target = None,
    }
}

impl From<RehydrateUserError> for UpdateUserError {
    fn from(error: RehydrateUserError) -> Self {
        Self::InvalidUserState {
            source: box_error(error),
        }
    }
}

fn authorize_user_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateUserError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateUserError>()
}

impl From<UserRepositoryError> for UpdateUserError {
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
    use super::{UpdateUserCommand, UpdateUserError, UpdateUserHandler, UpdateUserUseCase};
    use common::patch_field::PatchField;
    use common::user_id::UserId;

    use crate::ports::{
        UserRepository, UserRepositoryError, UserRepositoryFactory, UserStorageVersion,
        VersionedUser,
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
    }

    impl UserRepositoryFactory<FakeTx> for FakeUserRepositoryFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserRepository + 'tx {
            FakeUserRepository {
                state: Arc::clone(&self.state),
            }
        }
    }

    fn changed_user_command(user_id: UserId) -> UpdateUserCommand {
        UpdateUserCommand {
            user_id,
            email: PatchField::Set(email("grace@example.com")),
            tier: PatchField::Set(UserTier::Pro),
            role: PatchField::Set(UserRole::Admin),
            prohibited_content_consent: PatchField::Set(true),
            stripe_customer_id: PatchField::Set(StripeCustomerId::from("cus_123")),
            ..Default::default()
        }
    }

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            ..Default::default()
        };

        assert!(command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_field_set() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            tier: PatchField::Set(UserTier::Pro),
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[test]
    fn should_report_non_empty_update_when_optional_field_cleared() {
        let command = UpdateUserCommand {
            user_id: UserId::new(),
            first_name: PatchField::Clear,
            ..Default::default()
        };

        assert!(!command.is_empty());
    }

    #[tokio::test]
    async fn should_update_user_patch_branches_and_skip_update_when_noop() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        )));
        let handler = UpdateUserHandler::new(uow.clone(), repo.clone());

        let changed = assert_ok(
            handler
                .execute(
                    &ctx(Principal::User(user_id)),
                    changed_user_command(user_id),
                )
                .await,
        );
        assert_eq!(email("grace@example.com"), changed.email);
        assert_eq!(1, lock(&repo.state).update_calls);
        assert_eq!(1, lock(&uow.state).commits);

        let noop = UpdateUserCommand {
            user_id,
            ..Default::default()
        };
        assert_ok(handler.execute(&ctx(Principal::User(user_id)), noop).await);
        assert_eq!(1, lock(&repo.state).update_calls);
        assert_eq!(2, lock(&uow.state).commits);
    }

    #[tokio::test]
    async fn should_fail_update_user_when_not_found_required_clear_or_repo_error_without_commit() {
        let user_id = UserId::new();
        let uow = FakeUnitOfWork::default();
        let repo = FakeUserRepositoryFactory::default();
        let handler = UpdateUserHandler::new(uow.clone(), repo.clone());
        assert_error(
            handler
                .execute(
                    &ctx(Principal::System),
                    UpdateUserCommand {
                        user_id,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateUserError::UserNotFound),
        );
        assert_eq!(0, lock(&uow.state).commits);

        lock(&repo.state).user = Some(versioned(user_with(
            user_id,
            "ada@example.com",
            UserRole::User,
            UserTier::Free,
        )));
        for command in [
            UpdateUserCommand {
                user_id,
                email: PatchField::Clear,
                ..Default::default()
            },
            UpdateUserCommand {
                user_id,
                tier: PatchField::Clear,
                ..Default::default()
            },
            UpdateUserCommand {
                user_id,
                role: PatchField::Clear,
                ..Default::default()
            },
        ] {
            assert_error(
                handler.execute(&ctx(Principal::System), command).await,
                |error| {
                    matches!(
                        error,
                        UpdateUserError::EmailRequired
                            | UpdateUserError::TierRequired
                            | UpdateUserError::RoleRequired
                    )
                },
            );
        }
        assert_eq!(0, lock(&repo.state).update_calls);

        lock(&repo.state).update_error = Some(RepoErrorKind::TemporarilyUnavailable);
        assert_error(
            handler
                .execute(&ctx(Principal::System), changed_user_command(user_id))
                .await,
            |error| matches!(error, UpdateUserError::TemporarilyUnavailable { .. }),
        );
    }

    #[tokio::test]
    async fn should_map_begin_and_commit_failures_for_update_user() {
        let user_id = UserId::new();
        let begin_uow = FakeUnitOfWork::default();
        lock(&begin_uow.state).begin_error = true;
        assert_error(
            UpdateUserHandler::new(begin_uow, FakeUserRepositoryFactory::default())
                .execute(
                    &ctx(Principal::System),
                    UpdateUserCommand {
                        user_id,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateUserError::BeginTransactionFailed),
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
            UpdateUserHandler::new(commit_uow, repo)
                .execute(
                    &ctx(Principal::System),
                    UpdateUserCommand {
                        user_id,
                        ..Default::default()
                    },
                )
                .await,
            |error| matches!(error, UpdateUserError::CommitTransactionFailed),
        );
    }
}
