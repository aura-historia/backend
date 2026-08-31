use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::{
    partnership_application::PartnershipApplication,
    partnership_application_id::PartnershipApplicationId,
};
use user_service::ports::UserAdminReaderFactory;
#[derive(Debug, Clone, PartialEq)]
pub struct MarkPartnershipApplicationInReviewCommand {
    pub application_id: PartnershipApplicationId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct MarkPartnershipApplicationInReviewResult {
    pub application: PartnershipApplication,
}
#[derive(Debug, thiserror::Error)]
pub enum MarkPartnershipApplicationInReviewError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("partnership application is not reviewable")]
    ApplicationNotReviewable,
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
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
pub trait MarkPartnershipApplicationInReviewUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: MarkPartnershipApplicationInReviewCommand,
    ) -> Result<MarkPartnershipApplicationInReviewResult, MarkPartnershipApplicationInReviewError>;
}
pub struct MarkPartnershipApplicationInReviewHandler<U, A, R> {
    unit_of_work: U,
    applications: A,
    admins: R,
}
impl<U, A, R> MarkPartnershipApplicationInReviewHandler<U, A, R> {
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
> MarkPartnershipApplicationInReviewUseCase for MarkPartnershipApplicationInReviewHandler<U, A, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: MarkPartnershipApplicationInReviewCommand,
    ) -> Result<MarkPartnershipApplicationInReviewResult, MarkPartnershipApplicationInReviewError>
    {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| MarkPartnershipApplicationInReviewError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let mut application = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(command.application_id)
            .await?
            .ok_or(MarkPartnershipApplicationInReviewError::NotFound)?;
        application
            .value
            .mark_in_review()
            .map_err(|_| MarkPartnershipApplicationInReviewError::ApplicationNotReviewable)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&application.value, application.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| MarkPartnershipApplicationInReviewError::CommitTransactionFailed)?;
        Ok(MarkPartnershipApplicationInReviewResult { application })
    }
}
impl From<AdminAuthorizationError> for MarkPartnershipApplicationInReviewError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
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
impl From<PartnershipApplicationRepositoryError> for MarkPartnershipApplicationInReviewError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
