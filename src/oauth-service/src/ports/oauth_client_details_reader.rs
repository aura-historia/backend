use super::{OAuthClientReadError, OAuthClientView};
use credential_core::oauth_client_id::OAuthClientId;

#[async_trait::async_trait]
pub trait OAuthClientDetailsReader: Send + Sync {
    async fn find(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientView>, OAuthClientReadError>;
}
