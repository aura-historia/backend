use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::{
    partnership_application::PartnershipApplication,
    partnership_application_id::PartnershipApplicationId,
};
#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnershipApplicationCommand {
    pub application_id: PartnershipApplicationId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnershipApplicationResult {
    pub application: PartnershipApplication,
}
#[derive(Debug, thiserror::Error)]
pub enum WithdrawPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("partnership application is not withdrawable")]
    ApplicationNotWithdrawable,
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
pub trait WithdrawPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnershipApplicationCommand,
    ) -> Result<WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationError>;
}
pub struct WithdrawPartnershipApplicationHandler<U, A> {
    unit_of_work: U,
    applications: A,
}
impl<U, A> WithdrawPartnershipApplicationHandler<U, A> {
    pub fn new(unit_of_work: U, applications: A) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, A: PartnershipApplicationRepositoryFactory<U::Tx>>
    WithdrawPartnershipApplicationUseCase for WithdrawPartnershipApplicationHandler<U, A>
{
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnershipApplicationCommand,
    ) -> Result<WithdrawPartnershipApplicationResult, WithdrawPartnershipApplicationError> {
        let user = match context.principal {
            Principal::User(user) | Principal::DelegatedUser { user_id: user, .. } => user,
            Principal::Anonymous => {
                return Err(WithdrawPartnershipApplicationError::AuthenticatedActorRequired);
            }
            Principal::Service(_) | Principal::System => {
                return Err(WithdrawPartnershipApplicationError::Forbidden);
            }
        };
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawPartnershipApplicationError::BeginTransactionFailed)?;
        let mut application = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(user, command.application_id)
            .await?
            .ok_or(WithdrawPartnershipApplicationError::NotFound)?;
        application
            .value
            .withdraw()
            .map_err(|_| WithdrawPartnershipApplicationError::ApplicationNotWithdrawable)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&application.value, application.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| WithdrawPartnershipApplicationError::CommitTransactionFailed)?;
        Ok(WithdrawPartnershipApplicationResult { application })
    }
}
impl From<PartnershipApplicationRepositoryError> for WithdrawPartnershipApplicationError {
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
