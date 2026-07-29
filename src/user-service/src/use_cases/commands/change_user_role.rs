use crate::ports::{UserRepository, UserRepositoryError, UserRepositoryFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use user_core::{role::UserRole, user::User};

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleCommand {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangeUserRoleResult {
    pub user_id: UserId,
    pub role: UserRole,
}

#[derive(Debug, thiserror::Error)]
pub enum ChangeUserRoleError {
    #[error("authenticated actor required to change user role")]
    AuthenticatedActorRequired,
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

pub struct ChangeUserRoleHandler<U, R> {
    unit_of_work: U,
    users: R,
}

impl<U, R> ChangeUserRoleHandler<U, R> {
    pub fn new(unit_of_work: U, users: R) -> Self {
        Self {
            unit_of_work,
            users,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ChangeUserRoleUseCase for ChangeUserRoleHandler<U, R>
where
    U: UnitOfWork,
    R: UserRepositoryFactory<U::Tx>,
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
        context
            .principal
            .require_authenticated()
            .map_err(|_| ChangeUserRoleError::AuthenticatedActorRequired)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ChangeUserRoleError::BeginTransactionFailed)?;
        let common::versioned::Versioned {
            value: mut user,
            version,
        } = self
            .users
            .in_transaction(&mut tx)
            .find_by_id(command.user_id)
            .await?
            .ok_or(ChangeUserRoleError::UserNotFound)?;

        let outcome = user.change_role(command.role);
        if outcome.changed() {
            self.users
                .in_transaction(&mut tx)
                .update(&user, version)
                .await?;
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
            user_id: user.id(),
            role: user.account().role,
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
