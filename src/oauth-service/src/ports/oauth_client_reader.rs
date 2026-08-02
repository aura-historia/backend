use super::oauth_client_repository::OAuthClientRepositoryError;
use common::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;

#[async_trait::async_trait]
pub trait OAuthClientReader: Send + Sync {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClient>, OAuthClientRepositoryError>;
    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError>;
}
