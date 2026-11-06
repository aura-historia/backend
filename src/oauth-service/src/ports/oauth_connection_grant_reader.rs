use super::OAuthClientReadError;
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use std::collections::HashSet;
use user_core::access_token::Scope;
use user_core::user_id::UserId;

/// What a user already granted a metadata document client, shown on its consent page.
#[async_trait::async_trait]
pub trait OAuthConnectionGrantReader: Send + Sync {
    /// The scopes of the user's connection to the client; empty without a connection.
    async fn granted_scopes(
        &self,
        user_id: UserId,
        client_id: &MetadataDocumentClientId,
    ) -> Result<HashSet<Scope>, OAuthClientReadError>;
}
