use crate::ports::{UserAccountReadError, UserAccountReader, UserAccountReaderFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::transaction::{Transaction, UnitOfWork};
use common::{
    currency::domain::Currency, language::domain::Language,
    measurement_unit::domain::MeasurementUnit, stripe_customer_id::StripeCustomerId,
    user_id::UserId,
};
use geo::core::address::{GeoAddress, StructuredAddress};
use serde_email::Email;
use user_core::{first_name::FirstName, last_name::LastName, role::UserRole, tier::UserTier};

#[derive(Debug, Clone, PartialEq)]
pub enum GetUserRequest {
    ById(UserId),
    ByEmail(Email),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserDetailsView {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<FirstName>,
    pub last_name: Option<LastName>,
    pub language: Option<Language>,
    pub currency: Option<Currency>,
    pub measurement_unit: Option<MeasurementUnit>,
    pub prohibited_content_consent: bool,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
    pub structured_address: Option<StructuredAddress>,
    pub geo_address: Option<GeoAddress>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetUserError {
    #[error("user not found")]
    NotFound,
    #[error("temporary user account read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user account read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user account read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin get user transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get user transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetUserUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetUserRequest,
    ) -> Result<UserDetailsView, GetUserError>;
}

pub struct GetUserHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetUserHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetUserUseCase for GetUserHandler<U, R>
where
    U: UnitOfWork,
    R: UserAccountReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_user",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetUserRequest,
    ) -> Result<UserDetailsView, GetUserError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetUserError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .find_account(&request)
            .await?
            .ok_or(GetUserError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetUserError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserAccountReadError> for GetUserError {
    fn from(error: UserAccountReadError) -> Self {
        match error {
            UserAccountReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserAccountReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserAccountReadError::Internal { source } => Self::Internal { source },
        }
    }
}
