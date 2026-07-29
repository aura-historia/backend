use crate::ports::{UserRepository, UserRepositoryError, UserRepositoryFactory};
use common::error::boxed::{BoxError, box_error};
use common::operation_context::OperationContext;
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
        context
            .principal
            .require_authenticated()
            .map_err(|_| CreateUserError::AuthenticatedActorRequired)?;
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

        self.users.in_transaction(&mut tx).insert(&user).await?;

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
