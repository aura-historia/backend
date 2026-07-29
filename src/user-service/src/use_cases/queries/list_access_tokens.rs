use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use crate::use_cases::queries::get_access_token::AccessTokenView;
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensRequest {
    pub user_id: UserId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListAccessTokensResult {
    pub items: Vec<AccessTokenView>,
}

#[derive(Debug, thiserror::Error)]
pub enum ListAccessTokensError {
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token store failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ListAccessTokensUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListAccessTokensRequest,
    ) -> Result<ListAccessTokensResult, ListAccessTokensError>;
}

pub struct ListAccessTokensHandler<S> {
    store: S,
}

impl<S> ListAccessTokensHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> ListAccessTokensUseCase for ListAccessTokensHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "list_access_tokens",
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
        request: ListAccessTokensRequest,
    ) -> Result<ListAccessTokensResult, ListAccessTokensError> {
        let items = self
            .store
            .list_for_user(&request.user_id)
            .await?
            .into_iter()
            .map(AccessTokenView::from)
            .collect();

        Ok(ListAccessTokensResult { items })
    }
}

impl From<AccessTokenStoreError> for ListAccessTokensError {
    fn from(error: AccessTokenStoreError) -> Self {
        match error {
            AccessTokenStoreError::Conflict { source } => Self::Conflict { source },
            AccessTokenStoreError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenStoreError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenStoreError::Internal { source } => Self::Internal { source },
        }
    }
}
