use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use user_service::ports::UserAdminReaderFactory;
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListAdminPartnershipApplicationsRequest;
#[derive(Debug, Clone, PartialEq)]
pub struct ListAdminPartnershipApplicationsResult {
    pub items: Vec<PartnershipApplicationView>,
}
#[derive(Debug, thiserror::Error)]
pub enum ListAdminPartnershipApplicationsError {
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
pub trait ListAdminPartnershipApplicationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsError>;
}
pub struct ListAdminPartnershipApplicationsHandler<U, A, R> {
    unit_of_work: U,
    reader: A,
    admins: R,
}
impl<U, A, R> ListAdminPartnershipApplicationsHandler<U, A, R> {
    pub fn new(unit_of_work: U, reader: A, admins: R) -> Self {
        Self {
            unit_of_work,
            reader,
            admins,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, A: PartnershipApplicationReaderFactory<U::Tx>, R: UserAdminReaderFactory<U::Tx>>
    ListAdminPartnershipApplicationsUseCase for ListAdminPartnershipApplicationsHandler<U, A, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        _: ListAdminPartnershipApplicationsRequest,
    ) -> Result<ListAdminPartnershipApplicationsResult, ListAdminPartnershipApplicationsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListAdminPartnershipApplicationsError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let items = self.reader.in_transaction(&mut tx).list_all().await?;
        tx.commit()
            .await
            .map_err(|_| ListAdminPartnershipApplicationsError::CommitTransactionFailed)?;
        Ok(ListAdminPartnershipApplicationsResult { items })
    }
}
impl From<AdminAuthorizationError> for ListAdminPartnershipApplicationsError {
    fn from(v: AdminAuthorizationError) -> Self {
        match v {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<PartnershipApplicationReadError> for ListAdminPartnershipApplicationsError {
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
