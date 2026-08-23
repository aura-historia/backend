use super::oauth_client_repository::OAuthClientRepositoryError;
use oauth_core::client::OAuthClient;

#[async_trait::async_trait]
pub trait OAuthClientReader: Send + Sync {
    async fn list(&self) -> Result<Vec<OAuthClient>, OAuthClientRepositoryError>;
}
