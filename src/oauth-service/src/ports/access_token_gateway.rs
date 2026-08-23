use application::error::BoxError;
use credential_core::oauth_client_id::OAuthClientId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenOrigin, RawAccessToken, Scope};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct IssuedAccessToken {
    pub raw: RawAccessToken,
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub user_id: UserId,
    pub origin: AccessTokenOrigin,
    pub issued_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewOAuthAccessToken {
    pub user_id: UserId,
    pub client_id: OAuthClientId,
    pub client_name: String,
    pub scopes: HashSet<Scope>,
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthAccessTokenGatewayError {
    #[error("access token not found")]
    NotFound,
    #[error("access token expired")]
    Expired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("temporary access token failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait OAuthAccessTokenGateway: Send + Sync {
    async fn issue(
        &self,
        token: NewOAuthAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError>;
    async fn delete_raw(&self, raw: &RawAccessToken) -> Result<(), OAuthAccessTokenGatewayError>;
    async fn find_raw(
        &self,
        raw: &RawAccessToken,
    ) -> Result<IssuedAccessToken, OAuthAccessTokenGatewayError>;
}
