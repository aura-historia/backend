use crate::ports::{
    UserPartnerShopsReadError, UserPartnerShopsReader, UserPartnerShopsReaderFactory,
};
use common::error::boxed::BoxError;
use common::operation_context::{OperationContext, Principal};
use common::transaction::{Transaction, UnitOfWork};
use common::{shop_id::ShopId, shop_name::ShopName, shop_slug_id::ShopSlugId, user_id::UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnerShopSummary {
    pub shop_id: ShopId,
    pub shop_slug_id: ShopSlugId,
    pub name: ShopName,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListPartnerShopsResult {
    pub user_id: UserId,
    pub items: Vec<PartnerShopSummary>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListPartnerShopsError {
    #[error("user not found")]
    UserNotFound,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary user partner shops read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user partner shops read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user partner shops read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin list partner shops transaction")]
    BeginTransactionFailed,
    #[error("failed to commit list partner shops transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait ListPartnerShopsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError>;
}

pub struct ListPartnerShopsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> ListPartnerShopsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> ListPartnerShopsUseCase for ListPartnerShopsHandler<U, R>
where
    U: UnitOfWork,
    R: UserPartnerShopsReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "list_partner_shops",
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
        request: ListPartnerShopsRequest,
    ) -> Result<ListPartnerShopsResult, ListPartnerShopsError> {
        authorize_list(context, request.user_id)?;
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ListPartnerShopsError::BeginTransactionFailed)?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .list_partner_shops(&request)
            .await?;
        tx.commit()
            .await
            .map_err(|_| ListPartnerShopsError::CommitTransactionFailed)?;

        Ok(result)
    }
}

fn authorize_list(
    context: &OperationContext,
    requested_user_id: UserId,
) -> Result<(), ListPartnerShopsError> {
    match &context.principal {
        Principal::User(user_id) if *user_id == requested_user_id => Ok(()),
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous | Principal::User(_) => Err(ListPartnerShopsError::Forbidden),
    }
}

impl From<UserPartnerShopsReadError> for ListPartnerShopsError {
    fn from(error: UserPartnerShopsReadError) -> Self {
        match error {
            UserPartnerShopsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserPartnerShopsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            UserPartnerShopsReadError::Internal { source } => Self::Internal { source },
        }
    }
}
