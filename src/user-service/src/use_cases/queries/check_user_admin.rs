use crate::ports::{UserAdminReadError, UserAdminReaderFactory};
use crate::use_cases::authorization::{RequireAdminActorError, require_admin_actor};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckUserAdminRequest;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CheckUserAdminResult;

#[derive(Debug, thiserror::Error)]
pub enum CheckUserAdminError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user admin read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user admin read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user admin read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin check user admin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit check user admin transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait CheckUserAdminUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError>;
}

pub struct CheckUserAdminHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> CheckUserAdminHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> CheckUserAdminUseCase for CheckUserAdminHandler<U, R>
where
    U: UnitOfWork,
    R: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "check_user_admin",
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
        _request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CheckUserAdminError::BeginTransactionFailed)?;
        {
            let mut reader = self.reader.in_transaction(&mut tx);
            require_admin_actor(context, &mut reader).await?;
        }
        tx.commit()
            .await
            .map_err(|_| CheckUserAdminError::CommitTransactionFailed)?;

        Ok(CheckUserAdminResult)
    }
}

impl From<RequireAdminActorError> for CheckUserAdminError {
    fn from(error: RequireAdminActorError) -> Self {
        match error {
            RequireAdminActorError::AuthenticationRequired => Self::AuthenticatedActorRequired,
            RequireAdminActorError::Forbidden => Self::Forbidden,
            RequireAdminActorError::UserAdminRead(error) => error.into(),
        }
    }
}

impl From<UserAdminReadError> for CheckUserAdminError {
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
