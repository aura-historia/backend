use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use user_core::user_id::UserId;
#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnPartnershipApplicationsRequest {
    pub user_id: UserId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListOwnPartnershipApplicationsResult {
    pub items: Vec<PartnershipApplicationView>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListOwnPartnershipApplicationsError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid read model")]
    InvalidReadModel {
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
pub trait ListOwnPartnershipApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnPartnershipApplicationsRequest,
    ) -> Result<ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsError>;
}
pub struct ListOwnPartnershipApplicationsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}
impl<U, R> ListOwnPartnershipApplicationsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationReaderFactory<U::Tx>>
    ListOwnPartnershipApplicationsUseCase for ListOwnPartnershipApplicationsHandler<U, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListOwnPartnershipApplicationsRequest,
    ) -> Result<ListOwnPartnershipApplicationsResult, ListOwnPartnershipApplicationsError> {
        match context.principal {
            Principal::User(id) | Principal::DelegatedUser { user_id: id, .. }
                if id == request.user_id => {}
            Principal::Anonymous => {
                return Err(ListOwnPartnershipApplicationsError::AuthenticatedActorRequired);
            }
            _ => return Err(ListOwnPartnershipApplicationsError::Forbidden),
        }
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListOwnPartnershipApplicationsError::BeginTransactionFailed)?;
        let items = self
            .reader
            .in_transaction(&mut tx)
            .list_by_user(request.user_id)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListOwnPartnershipApplicationsError::CommitTransactionFailed)?;
        Ok(ListOwnPartnershipApplicationsResult { items })
    }
}
impl From<PartnershipApplicationReadError> for ListOwnPartnershipApplicationsError {
    fn from(v: PartnershipApplicationReadError) -> Self {
        match v {
            PartnershipApplicationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PartnershipApplicationReadError::Internal { source } => Self::Internal { source },
        }
    }
}
