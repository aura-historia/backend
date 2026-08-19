use crate::ports::{UserDetailsView, UserRepository, UserRepositoryError, UserRepositoryFactory};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;
use user_core::user::AssociateStripeCustomerIdError;

#[derive(Debug, Clone, PartialEq)]
pub struct AssociateUserStripeCustomerIdCommand {
    pub user_id: UserId,
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssociateUserStripeCustomerIdResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum AssociateUserStripeCustomerIdError {
    #[error("authenticated actor required to associate user Stripe customer id")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    UserNotFound,
    #[error("user already has a different Stripe customer")]
    DifferentStripeCustomerAlreadyAssociated,
    #[error("concurrent user update")]
    ConcurrencyConflict,
    #[error("user Stripe customer already exists")]
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
    #[error("failed to begin user Stripe customer association transaction")]
    BeginTransactionFailed {
        #[source]
        source: application::transaction::TransactionError,
    },
    #[error("failed to commit user Stripe customer association transaction")]
    CommitTransactionFailed {
        #[source]
        source: application::transaction::TransactionError,
    },
}

#[async_trait::async_trait]
pub trait AssociateUserStripeCustomerIdUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssociateUserStripeCustomerIdCommand,
    ) -> Result<AssociateUserStripeCustomerIdResult, AssociateUserStripeCustomerIdError>;
}

pub struct AssociateUserStripeCustomerIdHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> AssociateUserStripeCustomerIdHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> AssociateUserStripeCustomerIdUseCase for AssociateUserStripeCustomerIdHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "associate_user_stripe_customer_id",
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
        command: AssociateUserStripeCustomerIdCommand,
    ) -> Result<AssociateUserStripeCustomerIdResult, AssociateUserStripeCustomerIdError> {
        context
            .require()
            .credential_capability(CredentialCapability::UsersWrite)
            .service_or_system()
            .authorize::<AssociateUserStripeCustomerIdError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            AssociateUserStripeCustomerIdError::BeginTransactionFailed { source }
        })?;
        let mut users = self.users.in_transaction(&mut tx);
        let common::versioned::Versioned {
            value: mut user,
            version,
        } = users
            .find_by_id(command.user_id)
            .await?
            .ok_or(AssociateUserStripeCustomerIdError::UserNotFound)?;

        let outcome = user
            .associate_stripe_customer_id(command.stripe_customer_id)
            .map_err(|error| match error {
                AssociateStripeCustomerIdError::DifferentCustomerAlreadyAssociated => {
                    AssociateUserStripeCustomerIdError::DifferentStripeCustomerAlreadyAssociated
                }
            })?;
        if outcome.changed() {
            user = users.update(&user, version).await?.value;
        }
        drop(users);

        tx.commit().await.map_err(|source| {
            AssociateUserStripeCustomerIdError::CommitTransactionFailed { source }
        })?;

        tracing::info!(
            event = "user.stripe_customer_associated",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(AssociateUserStripeCustomerIdResult {
            view: UserDetailsView::from(&user),
        })
    }
}

impl From<OperationAuthorizationError> for AssociateUserStripeCustomerIdError {
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

impl From<UserRepositoryError> for AssociateUserStripeCustomerIdError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source }
            | UserRepositoryError::Internal { source } => Self::Internal { source },
            UserRepositoryError::StripeCustomerConflict { source } => {
                Self::StripeCustomerConflict { source }
            }
            UserRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}
