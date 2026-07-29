use crate::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::pagination::cursor::Cursor;
use common::sort::Sort;
use common::transaction::{Transaction, UnitOfWork};
use common::{stripe_customer_id::StripeCustomerId, user_id::UserId};
use serde_email::Email;
use serde_json::Value;
use user_core::{role::UserRole, tier::UserTier, user_search::UserSearch};

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersRequest {
    pub search: UserSearch,
    pub sort: Option<Sort<user_core::sort_user_field::SortUserField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UserSummary {
    pub user_id: UserId,
    pub email: Email,
    pub first_name: Option<user_core::first_name::FirstName>,
    pub last_name: Option<user_core::last_name::LastName>,
    pub tier: UserTier,
    pub role: UserRole,
    pub stripe_customer_id: Option<StripeCustomerId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchUsersResult {
    pub items: Vec<UserSummary>,
    pub cursor: Cursor<Value>,
    pub total: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchUsersError {
    #[error("temporary user search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid user search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal user search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search users transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search users transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait SearchUsersUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError>;
}

pub struct SearchUsersHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchUsersHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchUsersUseCase for SearchUsersHandler<U, R>
where
    U: UnitOfWork,
    R: UserSearchReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_users",
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
        request: SearchUsersRequest,
    ) -> Result<SearchUsersResult, SearchUsersError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| SearchUsersError::BeginTransactionFailed)?;
        let result = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|_| SearchUsersError::CommitTransactionFailed)?;

        Ok(result)
    }
}

impl From<UserSearchReadError> for SearchUsersError {
    fn from(error: UserSearchReadError) -> Self {
        match error {
            UserSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            UserSearchReadError::InvalidReadModel { source } => Self::InvalidReadModel { source },
            UserSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}
