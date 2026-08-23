use crate::ports::{
    UserDetailsView, UserRepository, UserRepositoryError, UserRepositoryFactory,
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};
use application::error::BoxError;
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::tier::UserTier;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyStripeSubscriptionTarget {
    User(UserId),
    StripeCustomer(StripeCustomerId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyStripeSubscriptionCommand {
    pub target: ApplyStripeSubscriptionTarget,
    pub tier: UserTier,
    pub associate_stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyStripeSubscriptionResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyStripeSubscriptionError {
    #[error("authenticated actor required to apply Stripe subscription")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user stripe customer already exists")]
    StripeCustomerConflict {
        #[source]
        source: BoxError,
    },
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
    #[error("failed to begin apply Stripe subscription transaction")]
    BeginTransactionFailed,
    #[error("failed to commit apply Stripe subscription transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ApplyStripeSubscriptionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ApplyStripeSubscriptionCommand,
    ) -> Result<ApplyStripeSubscriptionResult, ApplyStripeSubscriptionError>;
}

pub struct ApplyStripeSubscriptionHandler<U, R, E> {
    unit_of_work: U,
    users: R,
    tier_entitlements: E,
}

impl<U, R, E> ApplyStripeSubscriptionHandler<U, R, E> {
    pub fn new(unit_of_work: U, users: R, tier_entitlements: E) -> Self {
        Self {
            unit_of_work,
            users,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E> ApplyStripeSubscriptionUseCase for ApplyStripeSubscriptionHandler<U, R, E>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
    E: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "apply_stripe_subscription",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ApplyStripeSubscriptionCommand,
    ) -> Result<ApplyStripeSubscriptionResult, ApplyStripeSubscriptionError> {
        context
            .require()
            .service_or_system()
            .authorize::<ApplyStripeSubscriptionError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApplyStripeSubscriptionError::BeginTransactionFailed)?;

        let user_id = match &command.target {
            ApplyStripeSubscriptionTarget::User(user_id) => *user_id,
            ApplyStripeSubscriptionTarget::StripeCustomer(stripe_customer_id) => self
                .users
                .in_transaction(&mut tx)
                .find_by_stripe_customer_id(stripe_customer_id)
                .await?
                .map(|user| user.value.id())
                .ok_or(ApplyStripeSubscriptionError::UserNotFound)?,
        };

        self.tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(user_id)
            .await?
            .ok_or(ApplyStripeSubscriptionError::UserNotFound)?;

        let domain_primitives::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(user_id)
            .await?
            .ok_or(ApplyStripeSubscriptionError::UserNotFound)?;

        let tier_changed = user.change_tier(command.tier).changed();
        let customer_changed = command
            .associate_stripe_customer_id
            .map(|stripe_customer_id| {
                user.change_stripe_customer_id(Some(stripe_customer_id))
                    .changed()
            })
            .unwrap_or(false);

        if tier_changed || customer_changed {
            user = self
                .users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?
                .value;
        }
        if tier_changed {
            self.tier_entitlements
                .in_transaction(&mut tx)
                .reconcile_for_tier(user_id, user.account().tier)
                .await?;
        }

        tx.commit()
            .await
            .map_err(|_| ApplyStripeSubscriptionError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.stripe_subscription_applied",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            tier = ?user.account().tier,
            tier_changed,
            customer_changed,
            outcome = "success",
        );

        Ok(ApplyStripeSubscriptionResult {
            view: UserDetailsView::from(&user),
        })
    }
}

impl From<OperationAuthorizationError> for ApplyStripeSubscriptionError {
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

impl From<UserTierEntitlementsError> for ApplyStripeSubscriptionError {
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

impl From<UserRepositoryError> for ApplyStripeSubscriptionError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::StripeCustomerConflict { source } => {
                Self::StripeCustomerConflict { source }
            }
            UserRepositoryError::EmailConflict { source }
            | UserRepositoryError::Internal { source } => Self::Internal { source },
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{UserRepository, UserStorageVersion, VersionedUser};
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use domain_primitives::versioned::Versioned;
    use serde_email::Email;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::role::UserRole;
    use user_core::user::{NewUser, User, UserAccount, UserPreferences, UserProfile};

    #[derive(Default)]
    struct State {
        user: Option<VersionedUser>,
        begins: usize,
        commits: usize,
        find_by_customer_calls: usize,
        lock_calls: usize,
        update_calls: usize,
        reconcile_calls: usize,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);

    struct FakeTx(Fakes);
    struct FakeUsers(Fakes);
    struct FakeEntitlements(Fakes);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn user(user_id: UserId, tier: UserTier, customer: Option<&str>) -> VersionedUser {
        let email = match Email::try_from("ada@example.com") {
            Ok(email) => email,
            Err(error) => panic!("test email must be valid: {error}"),
        };
        let user = match User::create(NewUser {
            id: user_id,
            email,
            profile: UserProfile::default(),
            preferences: UserPreferences::default(),
            account: UserAccount {
                tier,
                role: UserRole::User,
                stripe_customer_id: customer.map(StripeCustomerId::from),
            },
        }) {
            Ok(user) => user,
            Err(error) => panic!("test user must be valid: {error}"),
        };
        Versioned::new(user, UserStorageVersion::INITIAL)
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), application::transaction::TransactionError> {
            lock(&self.0.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(self.clone()))
        }
    }

    #[async_trait::async_trait]
    impl UserRepository for FakeUsers {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(lock(&self.0.0)
                .user
                .clone()
                .filter(|user| user.value.id() == user_id))
        }

        async fn find_by_email(
            &mut self,
            _email: &Email,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            Ok(None)
        }

        async fn find_by_stripe_customer_id(
            &mut self,
            customer: &StripeCustomerId,
        ) -> Result<Option<VersionedUser>, UserRepositoryError> {
            let mut state = lock(&self.0.0);
            state.find_by_customer_calls += 1;
            Ok(state
                .user
                .clone()
                .filter(|user| user.value.account().stripe_customer_id.as_ref() == Some(customer)))
        }

        async fn insert(&mut self, _user: &User) -> Result<VersionedUser, UserRepositoryError> {
            panic!("not used by apply Stripe subscription")
        }

        async fn insert_if_absent(
            &mut self,
            _user: &User,
        ) -> Result<crate::ports::UserInsertOutcome, UserRepositoryError> {
            panic!("not used by apply Stripe subscription")
        }

        async fn update(
            &mut self,
            user: &User,
            _expected_version: UserStorageVersion,
        ) -> Result<VersionedUser, UserRepositoryError> {
            let mut state = lock(&self.0.0);
            state.update_calls += 1;
            let persisted = Versioned::new(user.clone(), UserStorageVersion::INITIAL);
            state.user = Some(persisted.clone());
            Ok(persisted)
        }

        async fn delete_by_id(&mut self, _user_id: UserId) -> Result<bool, UserRepositoryError> {
            panic!("not used by apply Stripe subscription")
        }
    }

    impl UserRepositoryFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserRepository + 'tx {
            FakeUsers(self.clone())
        }
    }

    #[async_trait::async_trait]
    impl UserTierEntitlements for FakeEntitlements {
        async fn lock_user_tier(
            &mut self,
            user_id: UserId,
        ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
            let mut state = lock(&self.0.0);
            state.lock_calls += 1;
            Ok(state
                .user
                .as_ref()
                .filter(|user| user.value.id() == user_id)
                .map(|user| user.value.account().tier))
        }

        async fn reconcile_for_tier(
            &mut self,
            _user_id: UserId,
            _tier: UserTier,
        ) -> Result<(), UserTierEntitlementsError> {
            lock(&self.0.0).reconcile_calls += 1;
            Ok(())
        }
    }

    impl UserTierEntitlementsFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl UserTierEntitlements + 'tx {
            FakeEntitlements(self.clone())
        }
    }

    #[tokio::test]
    async fn should_reject_non_system_principal_before_starting_stripe_subscription_transaction() {
        let fakes = Fakes::default();
        let result =
            ApplyStripeSubscriptionHandler::new(fakes.clone(), fakes.clone(), fakes.clone())
                .execute(
                    &context(Principal::Anonymous),
                    ApplyStripeSubscriptionCommand {
                        target: ApplyStripeSubscriptionTarget::User(UserId::new()),
                        tier: UserTier::Pro,
                        associate_stripe_customer_id: None,
                    },
                )
                .await;

        assert!(matches!(
            result,
            Err(ApplyStripeSubscriptionError::AuthenticatedActorRequired)
        ));
        assert_eq!(0, lock(&fakes.0).begins);
    }

    #[tokio::test]
    async fn should_apply_tier_and_customer_in_one_committed_transaction() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        lock(&fakes.0).user = Some(user(user_id, UserTier::Free, None));

        let result =
            ApplyStripeSubscriptionHandler::new(fakes.clone(), fakes.clone(), fakes.clone())
                .execute(
                    &context(Principal::System),
                    ApplyStripeSubscriptionCommand {
                        target: ApplyStripeSubscriptionTarget::User(user_id),
                        tier: UserTier::Pro,
                        associate_stripe_customer_id: Some(StripeCustomerId::from("cus_1")),
                    },
                )
                .await;

        assert!(matches!(result, Ok(ref result) if result.view.tier == UserTier::Pro));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.commits);
        assert_eq!(1, state.lock_calls);
        assert_eq!(1, state.update_calls);
        assert_eq!(1, state.reconcile_calls);
    }

    #[tokio::test]
    async fn should_find_by_customer_and_skip_write_for_repeated_subscription() {
        let user_id = UserId::new();
        let fakes = Fakes::default();
        lock(&fakes.0).user = Some(user(user_id, UserTier::Pro, Some("cus_1")));

        let result =
            ApplyStripeSubscriptionHandler::new(fakes.clone(), fakes.clone(), fakes.clone())
                .execute(
                    &context(Principal::System),
                    ApplyStripeSubscriptionCommand {
                        target: ApplyStripeSubscriptionTarget::StripeCustomer(
                            StripeCustomerId::from("cus_1"),
                        ),
                        tier: UserTier::Pro,
                        associate_stripe_customer_id: None,
                    },
                )
                .await;

        assert!(result.is_ok());
        let state = lock(&fakes.0);
        assert_eq!(1, state.find_by_customer_calls);
        assert_eq!(1, state.lock_calls);
        assert_eq!(0, state.update_calls);
        assert_eq!(0, state.reconcile_calls);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_not_commit_when_stripe_customer_has_no_user() {
        let fakes = Fakes::default();
        let result =
            ApplyStripeSubscriptionHandler::new(fakes.clone(), fakes.clone(), fakes.clone())
                .execute(
                    &context(Principal::System),
                    ApplyStripeSubscriptionCommand {
                        target: ApplyStripeSubscriptionTarget::StripeCustomer(
                            StripeCustomerId::from("cus_missing"),
                        ),
                        tier: UserTier::Free,
                        associate_stripe_customer_id: None,
                    },
                )
                .await;

        assert!(matches!(
            result,
            Err(ApplyStripeSubscriptionError::UserNotFound)
        ));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.find_by_customer_calls);
        assert_eq!(0, state.commits);
    }
}
