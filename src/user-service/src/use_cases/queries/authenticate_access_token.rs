use crate::ports::{AccessTokenStore, AccessTokenStoreError};
use common::error::boxed::BoxError;
use common::operation_context::OperationContext;
use common::user_id::UserId;
use std::collections::HashSet;
use user_core::access_token::{HashedRawAccessToken, Scope};

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenRequest {
    pub hashed_token: HashedRawAccessToken,
    pub required_scopes: HashSet<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthenticateAccessTokenResult {
    pub user_id: UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthenticateAccessTokenError {
    #[error("access token not found")]
    NotFound,
    #[error("access token expired")]
    Expired,
    #[error("access token lacks required scope")]
    InsufficientScope,
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
pub trait AuthenticateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError>;
}

pub struct AuthenticateAccessTokenHandler<S> {
    store: S,
}

impl<S> AuthenticateAccessTokenHandler<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl<S> AuthenticateAccessTokenUseCase for AuthenticateAccessTokenHandler<S>
where
    S: AccessTokenStore,
{
    #[tracing::instrument(
        name = "authenticate_access_token",
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
        request: AuthenticateAccessTokenRequest,
    ) -> Result<AuthenticateAccessTokenResult, AuthenticateAccessTokenError> {
        let access_token = self
            .store
            .find_by_hashed_token(&request.hashed_token)
            .await?
            .ok_or(AuthenticateAccessTokenError::NotFound)?;

        if access_token.is_expired() {
            return Err(AuthenticateAccessTokenError::Expired);
        }
        if !request
            .required_scopes
            .iter()
            .copied()
            .all(|scope| access_token.has_scope(scope))
        {
            return Err(AuthenticateAccessTokenError::InsufficientScope);
        }

        Ok(AuthenticateAccessTokenResult {
            user_id: access_token.user_id,
        })
    }
}

impl From<AccessTokenStoreError> for AuthenticateAccessTokenError {
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
