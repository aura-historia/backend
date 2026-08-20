use crate::ports::{
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserDetailsView,
};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GetOwnUserRequest;

#[derive(Debug, thiserror::Error)]
pub enum GetOwnUserError {
    #[error("authenticated actor required to get own user")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("user not found")]
    NotFound,
    #[error("temporary user account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user account read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user account read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get own user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get own user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetOwnUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnUserRequest,
    ) -> Result<UserDetailsView, GetOwnUserError>;
}

pub struct GetOwnUserHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetOwnUserHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetOwnUserUseCase for GetOwnUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_own_user",
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
        _request: GetOwnUserRequest,
    ) -> Result<UserDetailsView, GetOwnUserError> {
        let user_id = authorize_get_own_user(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(user_id));

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetOwnUserError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_by_id(user_id)
            .await?
            .ok_or(GetOwnUserError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetOwnUserError::CommitTransactionFailed)?;

        Ok(result)
    }
}

fn authorize_get_own_user(context: &OperationContext) -> Result<UserId, GetOwnUserError> {
    context
        .require()
        .credential_capability(CredentialCapability::UsersRead)
        .any_user()
        .authorize::<GetOwnUserError>()?;

    match context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(user_id),
        Principal::Anonymous => Err(GetOwnUserError::AuthenticatedActorRequired),
        Principal::Service(_) | Principal::System => Err(GetOwnUserError::Forbidden),
    }
}

impl From<OperationAuthorizationError> for GetOwnUserError {
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

impl From<UserAccountReadError> for GetOwnUserError {
    fn from(error: UserAccountReadError) -> Self {
        match error {
            UserAccountReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAccountReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAccountReadError::Internal { source } => Self::Internal { source },
        }
    }
}
