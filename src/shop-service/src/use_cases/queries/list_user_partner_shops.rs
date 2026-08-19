use crate::ports::{PartnerShopReadError, PartnerShopReader, PartnerShopReaderFactory};
use crate::use_cases::queries::search_shops::ShopSummary;
use application::transaction::{Transaction, UnitOfWork};
use common::error::boxed::BoxError;
use common::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListUserPartnerShopsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListUserPartnerShopsResult {
    pub user_id: UserId,
    pub items: Vec<ShopSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListUserPartnerShopsError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary partner shop read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid partner shop read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal partner shop read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list user partner shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit list user partner shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListUserPartnerShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListUserPartnerShopsRequest,
    ) -> Result<ListUserPartnerShopsResult, ListUserPartnerShopsError>;
}

pub struct ListUserPartnerShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> ListUserPartnerShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListUserPartnerShopsUseCase for ListUserPartnerShopsHandler<U, R>
where
    U: UnitOfWork,
    R: PartnerShopReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_user_partner_shops",
        skip_all,
        fields(
            user_id = %request.user_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListUserPartnerShopsRequest,
    ) -> Result<ListUserPartnerShopsResult, ListUserPartnerShopsError> {
        context
            .require()
            .credential_capability(CredentialCapability::PartnerShopsRead)
            .user(&request.user_id)
            .service_or_system()
            .authorize::<ListUserPartnerShopsError>()?;

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListUserPartnerShopsError::BeginTransactionFailed)?;

        let items = self
            .reader
            .in_transaction(&mut tx)
            .list_summaries_for_user(request.user_id)
            .await?;

        tx.commit()
            .await
            .map_err(|_| ListUserPartnerShopsError::CommitTransactionFailed)?;

        Ok(ListUserPartnerShopsResult {
            user_id: request.user_id,
            items,
        })
    }
}

impl From<OperationAuthorizationError> for ListUserPartnerShopsError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_)
            | OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<PartnerShopReadError> for ListUserPartnerShopsError {
    fn from(error: PartnerShopReadError) -> Self {
        match error {
            PartnerShopReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnerShopReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            PartnerShopReadError::Internal { source } => Self::Internal { source },
        }
    }
}
