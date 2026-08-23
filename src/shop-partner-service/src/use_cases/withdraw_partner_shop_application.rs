use crate::ports::{
    PartnerShopApplicationRepository, PartnerShopApplicationRepositoryError,
    PartnerShopApplicationRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use shop_partner_core::partner_shop_application::PartnerShopApplicationTransitionError;
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;
use shop_service::ports::{ShopRepository, ShopRepositoryError, ShopRepositoryFactory};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawPartnerShopApplicationCommand {
    pub user_id: UserId,
    pub application_id: PartnerShopApplicationId,
}

#[derive(Debug, thiserror::Error)]
pub enum WithdrawPartnerShopApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partner shop application not found")]
    NotFound,
    #[error("partner shop application is not withdrawable")]
    ApplicationNotWithdrawable,
    #[error("shop referenced by partner shop application not found")]
    ShopNotFound,
    #[error("new partner shop application references a non-draft shop")]
    DraftShopNotDiscardable,
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
pub trait WithdrawPartnerShopApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnerShopApplicationCommand,
    ) -> Result<(), WithdrawPartnerShopApplicationError>;
}

pub struct WithdrawPartnerShopApplicationHandler<U, A, S> {
    unit_of_work: U,
    applications: A,
    shops: S,
}

impl<U, A, S> WithdrawPartnerShopApplicationHandler<U, A, S> {
    pub fn new(unit_of_work: U, applications: A, shops: S) -> Self {
        Self {
            unit_of_work,
            applications,
            shops,
        }
    }
}

#[async_trait::async_trait]
impl<U, A, S> WithdrawPartnerShopApplicationUseCase
    for WithdrawPartnerShopApplicationHandler<U, A, S>
where
    U: UnitOfWork,
    A: PartnerShopApplicationRepositoryFactory<U::Tx>,
    S: ShopRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(name = "withdraw_partner_shop_application", skip_all, fields(user_id = %command.user_id, partner_shop_application_id = %command.application_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: WithdrawPartnerShopApplicationCommand,
    ) -> Result<(), WithdrawPartnerShopApplicationError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopApplicationsWrite)
            .user(&command.user_id)
            .service_or_system()
            .authorize::<WithdrawPartnerShopApplicationError>()?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawPartnerShopApplicationError::BeginTransactionFailed)?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_user_and_id(command.user_id, command.application_id)
            .await?
            .ok_or(WithdrawPartnerShopApplicationError::NotFound)?;

        versioned
            .value
            .withdraw()
            .map_err(withdraw_transition_error)?;

        if versioned.value.is_new_shop_application() {
            let mut shop = self
                .shops
                .in_transaction(&mut tx)
                .find_by_id(versioned.value.shop_id())
                .await?
                .ok_or(WithdrawPartnerShopApplicationError::ShopNotFound)?;
            let discarded = shop
                .shop
                .discard()
                .map_err(|_| WithdrawPartnerShopApplicationError::DraftShopNotDiscardable)?;
            if discarded == ChangeOutcome::Changed {
                self.shops
                    .in_transaction(&mut tx)
                    .update(&shop.shop, shop.version)
                    .await?;
            }
        }

        self.applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?;
        tx.commit()
            .await
            .map_err(|_| WithdrawPartnerShopApplicationError::CommitTransactionFailed)?;
        tracing::info!(
            event = "partner_shop_application.withdrawn",
            partner_shop_application_id = %command.application_id,
            outcome = "success",
        );
        Ok(())
    }
}

fn withdraw_transition_error(
    _: PartnerShopApplicationTransitionError,
) -> WithdrawPartnerShopApplicationError {
    WithdrawPartnerShopApplicationError::ApplicationNotWithdrawable
}

impl From<OperationAuthorizationError> for WithdrawPartnerShopApplicationError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopApplicationRepositoryError> for WithdrawPartnerShopApplicationError {
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

impl From<ShopRepositoryError> for WithdrawPartnerShopApplicationError {
    fn from(error: ShopRepositoryError) -> Self {
        match error {
            ShopRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ShopRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ShopRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ShopRepositoryError::SlugConflict { source }
            | ShopRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
