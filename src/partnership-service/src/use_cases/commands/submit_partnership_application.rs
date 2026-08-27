use crate::ports::*;
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, UnitOfWork},
};
use partnership_core::{
    partnership_application::{
        NewPartnershipApplication, PartnershipApplication, PartnershipProposal,
    },
    partnership_application_id::PartnershipApplicationId,
};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct SubmitPartnershipApplicationCommand {
    pub applicant_user_id: UserId,
    pub proposal: PartnershipProposal,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SubmitPartnershipApplicationResult {
    pub application: PartnershipApplication,
}
#[derive(Debug, thiserror::Error)]
pub enum SubmitPartnershipApplicationError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partnership application failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partnership application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership application failure")]
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
pub trait SubmitPartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitPartnershipApplicationCommand,
    ) -> Result<SubmitPartnershipApplicationResult, SubmitPartnershipApplicationError>;
}
pub struct SubmitPartnershipApplicationHandler<U, R> {
    unit_of_work: U,
    applications: R,
}
impl<U, R> SubmitPartnershipApplicationHandler<U, R> {
    pub fn new(unit_of_work: U, applications: R) -> Self {
        Self {
            unit_of_work,
            applications,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: PartnershipApplicationRepositoryFactory<U::Tx>>
    SubmitPartnershipApplicationUseCase for SubmitPartnershipApplicationHandler<U, R>
{
    #[tracing::instrument(name="submit_partnership_application", skip_all, fields(principal_type=context.principal.kind(), request_id=%context.request_id, correlation_id=%context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitPartnershipApplicationCommand,
    ) -> Result<SubmitPartnershipApplicationResult, SubmitPartnershipApplicationError> {
        authorize(context, command.applicant_user_id)?;
        let application = PartnershipApplication::submit(NewPartnershipApplication {
            id: PartnershipApplicationId::new(),
            applicant_user_id: command.applicant_user_id,
            proposal: command.proposal,
        });
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SubmitPartnershipApplicationError::BeginTransactionFailed)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .insert(&application)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| SubmitPartnershipApplicationError::CommitTransactionFailed)?;
        tracing::info!(event="partnership_application.submitted", partnership_application_id=%application.id(), actor_type=context.principal.kind(), outcome="success");
        Ok(SubmitPartnershipApplicationResult { application })
    }
}
fn authorize(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), SubmitPartnershipApplicationError> {
    match context.principal {
        Principal::Anonymous => Err(SubmitPartnershipApplicationError::AuthenticatedActorRequired),
        Principal::User(actor) | Principal::DelegatedUser { user_id: actor, .. }
            if actor == user_id =>
        {
            Ok(())
        }
        Principal::Service(_) | Principal::System => Ok(()),
        _ => Err(SubmitPartnershipApplicationError::Forbidden),
    }
}
impl From<PartnershipApplicationRepositoryError> for SubmitPartnershipApplicationError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::ConcurrencyConflict
            | PartnershipApplicationRepositoryError::Internal { source: _ } => Self::Internal {
                source: application::error::static_error(
                    "unexpected partnership application insert failure",
                ),
            },
        }
    }
}
