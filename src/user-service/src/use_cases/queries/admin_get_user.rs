use crate::ports::{
    UserAccountReadError, UserAccountReader, UserAccountReaderFactory, UserAdminReadError,
    UserAdminReaderFactory, UserDetailsView,
};
use crate::use_cases::authorization::{
    RequireAdminActorError, require_admin_actor, require_admin_actor_credential,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{CredentialCapability, OperationContext};
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminGetUserRequest {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminGetUserError {
    #[error("authenticated actor required to get user")]
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
    #[error("failed to begin admin get user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit admin get user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait AdminGetUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminGetUserRequest,
    ) -> Result<UserDetailsView, AdminGetUserError>;
}

pub struct AdminGetUserHandler<U, R, A> {
    unit_of_work: U,
    reader: R,
    admin_reader: A,
}

impl<U, R, A> AdminGetUserHandler<U, R, A> {
    pub fn new(unit_of_work: U, reader: R, admin_reader: A) -> Self {
        Self {
            unit_of_work,
            reader,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, A> AdminGetUserUseCase for AdminGetUserHandler<U, R, A>
where
    U: UnitOfWork,
    R: UserAccountReaderFactory<U::Tx>,
    A: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "admin_get_user",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AdminGetUserRequest,
    ) -> Result<UserDetailsView, AdminGetUserError> {
        require_admin_actor_credential(context, CredentialCapability::UsersRead)?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| AdminGetUserError::BeginTransactionFailed)?;
        {
            let mut reader = self.admin_reader.in_transaction(&mut tx);
            require_admin_actor(context, &mut reader).await?;
        }
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_by_id(request.user_id)
            .await?
            .ok_or(AdminGetUserError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| AdminGetUserError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<RequireAdminActorError> for AdminGetUserError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for AdminGetUserError {
    fn from(error: UserAdminReadError) -> Self {
        match error {
            UserAdminReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAdminReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAdminReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<UserAccountReadError> for AdminGetUserError {
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
