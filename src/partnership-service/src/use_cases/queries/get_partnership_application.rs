use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_application_id::PartnershipApplicationId;
use user_service::ports::UserAdminReaderFactory;
#[derive(Debug, Clone, PartialEq)]
pub struct GetPartnershipApplicationRequest {
    pub application_id: PartnershipApplicationId,
}
pub type GetPartnershipApplicationResult = PartnershipApplicationView;
#[derive(Debug, thiserror::Error)]
pub enum GetPartnershipApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait GetPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnershipApplicationRequest,
    ) -> Result<GetPartnershipApplicationResult, GetPartnershipApplicationError>;
}
pub struct GetPartnershipApplicationHandler<U, A, R> {
    unit_of_work: U,
    applications: A,
    admins: R,
}
impl<U, A, R> GetPartnershipApplicationHandler<U, A, R> {
    pub fn new(unit_of_work: U, applications: A, admins: R) -> Self {
        Self {
            unit_of_work,
            applications,
            admins,
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    A: PartnershipApplicationRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
> GetPartnershipApplicationUseCase for GetPartnershipApplicationHandler<U, A, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartnershipApplicationRequest,
    ) -> Result<GetPartnershipApplicationResult, GetPartnershipApplicationError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetPartnershipApplicationError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let app = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(request.application_id)
            .await?
            .ok_or(GetPartnershipApplicationError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(PartnershipApplicationView {
            id: app.value.id(),
            applicant_user_id: app.value.applicant_user_id(),
            state: app.value.state(),
            proposal: app.value.proposal().clone(),
            approval_result: app.value.approval_result(),
        })
    }
}
impl From<AdminAuthorizationError> for GetPartnershipApplicationError {
    fn from(v: AdminAuthorizationError) -> Self {
        match v {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<PartnershipApplicationRepositoryError> for GetPartnershipApplicationError {
    fn from(v: PartnershipApplicationRepositoryError) -> Self {
        match v {
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::Internal {
                source: application::error::static_error("unexpected concurrency"),
            },
            PartnershipApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
