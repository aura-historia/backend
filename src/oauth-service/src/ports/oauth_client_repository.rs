use application::error::BoxError;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;
use time::OffsetDateTime;

domain_primitives::version_newtype!(OAuthClientStorageVersion);

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedOAuthClient {
    pub value: OAuthClient,
    pub version: OAuthClientStorageVersion,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type VersionedOAuthClient = PersistedOAuthClient;

#[derive(Debug, thiserror::Error)]
pub enum OAuthClientRepositoryError {
    #[error("concurrent oauth client update")]
    ConcurrencyConflict,
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

#[async_trait::async_trait]
pub trait OAuthClientRepository: Send {
    async fn find_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<Option<VersionedOAuthClient>, OAuthClientRepositoryError>;

    async fn insert(
        &mut self,
        client: &OAuthClient,
    ) -> Result<PersistedOAuthClient, OAuthClientRepositoryError>;

    async fn update(
        &mut self,
        client: &OAuthClient,
        expected_version: OAuthClientStorageVersion,
    ) -> Result<PersistedOAuthClient, OAuthClientRepositoryError>;

    async fn delete_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<bool, OAuthClientRepositoryError>;
}

pub trait OAuthClientRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl OAuthClientRepository + 'tx;
}
