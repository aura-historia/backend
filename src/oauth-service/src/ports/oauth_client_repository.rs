use application::error::BoxError;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::Scope;

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

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientPatch {
    pub name: Option<oauth_core::client::OAuthClientName>,
    pub redirect_uris: Option<HashSet<url::Url>>,
    pub tos_uri: Option<url::Url>,
    pub policy_uri: Option<url::Url>,
    pub client_uri: Option<url::Url>,
    pub logo_uri: Option<url::Url>,
    pub scopes: Option<HashSet<Scope>>,
    pub updated: OffsetDateTime,
}

#[async_trait::async_trait]
pub trait OAuthClientRepository: Send + Sync {
    async fn find_by_client_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError>;

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
