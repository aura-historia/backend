use application::error::BoxError;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_core::metadata_document_client::ClientMetadataDocumentClaims;

/// Retrieves the Client ID Metadata Document published at a client's `client_id` URL.
///
/// Implementations own the outbound network policy and any caching. They never serve a
/// document after its cache lifetime ends; a failed refetch is an error.
#[async_trait::async_trait]
pub trait ClientMetadataDocumentSource: Send + Sync {
    async fn load(
        &self,
        client_id: &MetadataDocumentClientId,
    ) -> Result<ClientMetadataDocumentClaims, ClientMetadataDocumentLoadError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ClientMetadataDocumentLoadError {
    /// The document could not be retrieved within the outbound policy, including refused
    /// destinations, redirects, timeouts and unsuccessful responses.
    #[error("client metadata document is unavailable")]
    Unavailable {
        #[source]
        source: BoxError,
    },
    /// The response is not a JSON metadata document.
    #[error("client metadata document is malformed")]
    Malformed {
        #[source]
        source: BoxError,
    },
}
