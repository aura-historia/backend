use crate::ports::{
    UserStripeCustomerReadError, UserStripeCustomerReader, UserStripeCustomerReaderFactory,
};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use user_core::{role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq)]
pub struct FindUserByStripeCustomerIdRequest {
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserStripeLookupView {
    pub user_id: UserId,
    pub email: Email,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: StripeCustomerId,
}

#[derive(Debug, thiserror::Error)]
pub enum FindUserByStripeCustomerIdError {
    #[error("user not found")]
    NotFound,
    #[error("temporary user stripe customer lookup failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user stripe customer read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user stripe customer lookup failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin find user by stripe customer id transaction")]
    BeginTransactionFailed,
    #[error("failed to commit find user by stripe customer id transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait FindUserByStripeCustomerIdUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: FindUserByStripeCustomerIdRequest,
    ) -> Result<UserStripeLookupView, FindUserByStripeCustomerIdError>;
}

pub struct FindUserByStripeCustomerIdHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> FindUserByStripeCustomerIdHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> FindUserByStripeCustomerIdUseCase for FindUserByStripeCustomerIdHandler<U, R>
where
    U: UnitOfWork,
    R: UserStripeCustomerReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "find_user_by_stripe_customer_id",
        skip_all,
        fields(
            stripe_customer_id = %request.stripe_customer_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: FindUserByStripeCustomerIdRequest,
    ) -> Result<UserStripeLookupView, FindUserByStripeCustomerIdError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| FindUserByStripeCustomerIdError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_by_stripe_customer_id(&request)
            .await?
            .ok_or(FindUserByStripeCustomerIdError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| FindUserByStripeCustomerIdError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserStripeCustomerReadError> for FindUserByStripeCustomerIdError {
    fn from(error: UserStripeCustomerReadError) -> Self {
        match error {
            UserStripeCustomerReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserStripeCustomerReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            UserStripeCustomerReadError::Internal { source } => Self::Internal { source },
        }
    }
}
