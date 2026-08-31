use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::partnership_application_id::PartnershipApplicationId;
#[derive(Debug, Clone, PartialEq)]
pub struct GetOwnPartnershipApplicationRequest {
    pub application_id: PartnershipApplicationId,
}
pub type GetOwnPartnershipApplicationResult = PartnershipApplicationView;
#[derive(Debug, thiserror::Error)]
pub enum GetOwnPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
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
pub trait GetOwnPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnPartnershipApplicationRequest,
    ) -> Result<GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationError>;
}
pub struct GetOwnPartnershipApplicationHandler<U, R> {
    unit_of_work: U,
    applications: R,
}
impl<U, R> GetOwnPartnershipApplicationHandler<U, R> {
    pub fn new(unit_of_work: U, applications: R) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationRepositoryFactory<U::Tx>>
    GetOwnPartnershipApplicationUseCase for GetOwnPartnershipApplicationHandler<U, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetOwnPartnershipApplicationRequest,
    ) -> Result<GetOwnPartnershipApplicationResult, GetOwnPartnershipApplicationError> {
        let user = match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => id,
            Principal::Anonymous => {
                return Err(GetOwnPartnershipApplicationError::AuthenticatedActorRequired);
            }
            Principal::Service(_) | Principal::System => {
                return Err(GetOwnPartnershipApplicationError::Forbidden);
            }
        };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetOwnPartnershipApplicationError::BeginTransactionFailed)?;
        let app = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(user, request.application_id)
            .await?
            .ok_or(GetOwnPartnershipApplicationError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetOwnPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(PartnershipApplicationView {
            id: app.value.id(),
            applicant_user_id: app.value.applicant_user_id(),
            state: app.value.state(),
            proposal: app.value.proposal().clone(),
        })
    }
}
impl From<PartnershipApplicationRepositoryError> for GetOwnPartnershipApplicationError {
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
