use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::user_id::UserId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, Scope,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GetAccessTokenRequest {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenView {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAccessTokenError {
    #[error("access token not found")]
    NotFound,
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
pub trait GetAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAccessTokenRequest,
    ) -> Result<AccessTokenView, GetAccessTokenError>;
}

pub struct GetAccessTokenHandler<S> {
    store: S,
}

impl<S> GetAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> GetAccessTokenUseCase for GetAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "get_access_token",
        skip_all,
        fields(
            user_id = %request.user_id,
            access_token_id = %request.access_token_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAccessTokenRequest,
    ) -> Result<AccessTokenView, GetAccessTokenError> {
        let access_token = self
            .store
            .find_by_id(&request.user_id, &request.access_token_id)
            .await?
            .ok_or(GetAccessTokenError::NotFound)?;

        Ok(AccessTokenView::from(access_token))
    }
}

impl From<AccessToken> for AccessTokenView {
    fn from(access_token: AccessToken) -> Self {
        Self {
            user_id: access_token.user_id,
            access_token_id: access_token.id,
            name: access_token.name,
            scopes: access_token.scopes,
            origin: access_token.origin,
            expires: access_token.expires,
        }
    }
}

impl From<AccessTokenStoreError> for GetAccessTokenError {
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
