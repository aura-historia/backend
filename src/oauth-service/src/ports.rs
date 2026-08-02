use common::error::boxed::BoxError;
use common::oauth_client_id::OAuthClientId;
use common::user_id::UserId;
use oauth_core::authorization_code::{AuthorizationCode, OAuthAuthorizationCode};
use oauth_core::client::OAuthClient;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenOrigin, RawAccessToken, Scope};

#[derive(Debug, thiserror::Error)]
pub enum OAuthClientRepositoryError {
    #[error("oauth client already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary oauth client repository failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted oauth client state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal oauth client repository failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthCodeRepositoryError {
    #[error("oauth code already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary oauth code repository failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted oauth code state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal oauth code repository failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssuedAccessToken {
    pub raw: RawAccessToken,
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub user_id: UserId,
    pub origin: AccessTokenOrigin,
    pub created: OffsetDateTime,
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
pub trait OAuthClientRepository: Send + Sync {
    async fn insert(
        &self,
        client: OAuthClient,
        raw_secret: user_core::access_token::RawOAuthClientSecret,
    ) -> Result<(), OAuthClientRepositoryError>;
    async fn update(
        &self,
        client_id: &OAuthClientId,
        patch: OAuthClientPatch,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError>;
    async fn delete(&self, client_id: &OAuthClientId) -> Result<(), OAuthClientRepositoryError>;
}

#[async_trait::async_trait]
pub trait OAuthClientReader: Send + Sync {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError>;
    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientPatch {
    pub name: Option<oauth_core::client::OAuthClientName>,
    pub redirect_uris: Option<HashSet<url::Url>>,
    pub tos_uri: Option<url::Url>,
    pub policy_uri: Option<url::Url>,
    pub client_uri: Option<url::Url>,
    pub logo_uri: Option<url::Url>,
    pub scopes: Option<HashSet<Scope>>,
    pub updated_by: common::actor::domain::Actor,
    pub updated: OffsetDateTime,
}

#[async_trait::async_trait]
pub trait AuthorizationCodeRepository: Send + Sync {
    async fn insert(&self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;
    async fn find_by_code(
        &self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError>;
    async fn delete(&self, code: &OAuthAuthorizationCode) -> Result<(), OAuthCodeRepositoryError>;
}

#[async_trait::async_trait]
pub trait ThirdPartyExchangeCodeRepository: Send + Sync {
    async fn insert(
        &self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError>;
    async fn find_by_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError>;
    async fn delete(&self, code: &ThirdPartyExchangeCode) -> Result<(), OAuthCodeRepositoryError>;
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
