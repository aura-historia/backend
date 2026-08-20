use crate::ports::{UserDetailsView, UserRepository, UserRepositoryError, UserRepositoryFactory};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::stripe_customer_id::StripeCustomerId;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct SetUserStripeCustomerIdCommand {
    pub user_id: UserId,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SetUserStripeCustomerIdResult {
    pub view: UserDetailsView,
}

#[derive(Debug, thiserror::Error)]
pub enum SetUserStripeCustomerIdError {
    #[error("authenticated actor required to set user stripe customer id")]
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
    #[error("failed to begin set user stripe customer id transaction")]
    BeginTransactionFailed,
    #[error("failed to commit set user stripe customer id transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SetUserStripeCustomerIdUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SetUserStripeCustomerIdCommand,
    ) -> Result<SetUserStripeCustomerIdResult, SetUserStripeCustomerIdError>;
}

pub struct SetUserStripeCustomerIdHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> SetUserStripeCustomerIdHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SetUserStripeCustomerIdUseCase for SetUserStripeCustomerIdHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "set_user_stripe_customer_id",
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
        command: SetUserStripeCustomerIdCommand,
    ) -> Result<SetUserStripeCustomerIdResult, SetUserStripeCustomerIdError> {
        context
            .require()
            .credential_capability(CredentialCapability::UsersWrite)
            .service_or_system()
            .authorize::<SetUserStripeCustomerIdError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SetUserStripeCustomerIdError::BeginTransactionFailed)?;
        let mut users = self.users.in_transaction(&mut tx);
        let common::versioned::Versioned {
            value: mut user,
            version,
        } = users
            .find_by_id(command.user_id)
            .await?
            .ok_or(SetUserStripeCustomerIdError::UserNotFound)?;

        let outcome = user.change_stripe_customer_id(command.stripe_customer_id);
        if outcome.changed() {
            user = users.update(&user, version).await?.value;
        }
        drop(users);

        tx.commit()
            .await
            .map_err(|_| SetUserStripeCustomerIdError::CommitTransactionFailed)?;

        tracing::info!(
            event = "user.stripe_customer_id_set",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            user_id = %user.id(),
            changed = outcome.changed(),
            outcome = "success",
        );

        Ok(SetUserStripeCustomerIdResult {
            view: UserDetailsView::from(&user),
        })
    }
}

impl From<OperationAuthorizationError> for SetUserStripeCustomerIdError {
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

impl From<UserRepositoryError> for SetUserStripeCustomerIdError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            UserRepositoryError::EmailConflict { source } => Self::Internal { source },
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
