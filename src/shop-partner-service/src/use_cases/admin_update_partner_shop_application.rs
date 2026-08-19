use crate::admin_authorization::{AdminAuthorizationError, authorize_admin_actor};
use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{OperationAuthorizationError, OperationContext};
use common::partner_shop_application_id::PartnerShopApplicationId;
use shop_partner_core::partner_shop_application::{
    PartnerShopApplication, PartnerShopApplicationTransitionError,
};
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq)]
pub struct AdminGetPartnerShopApplicationForUpdateRequest {
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminMarkPartnerShopApplicationInReviewCommand {
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdminUpdatePartnerShopApplicationResult {
    pub application: PartnerShopApplication,
}

#[derive(Debug, thiserror::Error)]
pub enum AdminUpdatePartnerShopApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partner shop application not found")]
    NotFound,
    #[error("partner shop application is not reviewable")]
    ApplicationNotReviewable,
    #[error("concurrent partner shop application update")]
    ConcurrencyConflict,
    #[error("temporary partner shop application persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted partner shop application state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop application persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin partner shop application transaction")]
    BeginTransactionFailed,
    #[error("failed to commit partner shop application transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait AdminUpdatePartnerShopApplicationUseCase: Send + Sync {
    async fn mark_in_review(
        &self,
        context: &OperationContext,
        command: AdminMarkPartnerShopApplicationInReviewCommand,
    ) -> Result<AdminUpdatePartnerShopApplicationResult, AdminUpdatePartnerShopApplicationError>;
}

pub struct AdminUpdatePartnerShopApplicationHandler<U, A, R> {
    unit_of_work: U,
    applications: A,
    admin_reader: R,
}

impl<U, A, R> AdminUpdatePartnerShopApplicationHandler<U, A, R> {
    pub fn new(unit_of_work: U, applications: A, admin_reader: R) -> Self {
        Self {
            unit_of_work,
            applications,
            admin_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, R> AdminUpdatePartnerShopApplicationUseCase
    for AdminUpdatePartnerShopApplicationHandler<U, A, R>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    R: UserAdminReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "admin_mark_partner_shop_application_in_review", skip_all, fields(partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn mark_in_review(
        &self,
        context: &OperationContext,
        command: AdminMarkPartnerShopApplicationInReviewCommand,
    ) -> Result<AdminUpdatePartnerShopApplicationResult, AdminUpdatePartnerShopApplicationError>
    {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| AdminUpdatePartnerShopApplicationError::BeginTransactionFailed)?;
        authorize_admin_actor(context, &mut tx, &self.admin_reader).await?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id(command.application_id)
            .await?
            .ok_or(AdminUpdatePartnerShopApplicationError::NotFound)?;
        versioned
            .value
            .mark_in_review()
            .map_err(review_transition_error)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?
            .value;
        tx.commit()
            .await
            .map_err(|_| AdminUpdatePartnerShopApplicationError::CommitTransactionFailed)?;
        Ok(AdminUpdatePartnerShopApplicationResult { application })
    }
}

fn review_transition_error(
    _: PartnerShopApplicationTransitionError,
) -> AdminUpdatePartnerShopApplicationError {
    AdminUpdatePartnerShopApplicationError::ApplicationNotReviewable
}

impl From<AdminAuthorizationError> for AdminUpdatePartnerShopApplicationError {
    fn from(error: AdminAuthorizationError) -> Self {
        match error {
            AdminAuthorizationError::Forbidden
            | AdminAuthorizationError::Operation(
                OperationAuthorizationError::AuthenticationRequired(_),
            )
            | AdminAuthorizationError::Operation(OperationAuthorizationError::Forbidden)
            | AdminAuthorizationError::Operation(
                OperationAuthorizationError::InsufficientCapability { .. },
            ) => Self::Forbidden,
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

impl From<OperationAuthorizationError> for AdminUpdatePartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for AdminUpdatePartnerShopApplicationError {
    fn from(error: PartnerShopApplicationRepositoryError) -> Self {
        match error {
            PartnerShopApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnerShopApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnerShopApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
