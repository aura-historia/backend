use application::error::BoxError;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::oauth_connection_id::OAuthConnectionId;
use oauth_core::connection::OAuthConnection;
use user_core::user_id::UserId;

#[derive(Debug, thiserror::Error)]
pub enum OAuthConnectionRepositoryError {
    /// A concurrent request created the same connection or credential first.
    #[error("concurrent OAuth connection write")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary OAuth connection repository failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted OAuth connection state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal OAuth connection repository failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

/// Persists connections. Every write path locks the connection before touching its
/// refresh-token families, so concurrent requests serialize in one order.
#[async_trait::async_trait]
pub trait OAuthConnectionRepository: Send {
    async fn find_by_user_and_client_for_update(
        &mut self,
        user_id: UserId,
        client_id: &MetadataDocumentClientId,
    ) -> Result<Option<OAuthConnection>, OAuthConnectionRepositoryError>;

    async fn find_by_id_for_update(
        &mut self,
        connection_id: OAuthConnectionId,
    ) -> Result<Option<OAuthConnection>, OAuthConnectionRepositoryError>;

    async fn insert(
        &mut self,
        connection: &OAuthConnection,
    ) -> Result<(), OAuthConnectionRepositoryError>;

    async fn update(
        &mut self,
        connection: &OAuthConnection,
    ) -> Result<(), OAuthConnectionRepositoryError>;

    /// Ends the connection and with it every access token and refresh-token family it holds.
    /// Returns whether a connection existed.
    async fn delete(
        &mut self,
        user_id: UserId,
        connection_id: OAuthConnectionId,
    ) -> Result<bool, OAuthConnectionRepositoryError>;
}

pub trait OAuthConnectionRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl OAuthConnectionRepository + 'tx;
}
