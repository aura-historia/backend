use crate::ports::{UserAdminReadError, UserAdminReader, UserAdminReaderFactory};
use crate::use_cases::queries::get_user::GetUserRequest;
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::user_id::UserId;
use user_core::role::UserRole;

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CheckUserAdminResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckUserAdminError {
    #[error("user not found")]
    UserNotFound,
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
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| CheckUserAdminError::BeginTransactionFailed)?;
        let user = self
            .reader
            .in_transaction(&mut tx)
            .find_admin_view(&GetUserRequest::ById(request.user_id))
            .await?
            .ok_or(CheckUserAdminError::UserNotFound)?;
        tx.commit()
            .await
            .map_err(|_| CheckUserAdminError::CommitTransactionFailed)?;

        if user.role != UserRole::Admin {
            return Err(CheckUserAdminError::Forbidden);
        }

        Ok(CheckUserAdminResult {
            user_id: request.user_id,
        })
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
