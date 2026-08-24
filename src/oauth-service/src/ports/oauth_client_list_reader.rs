use super::{OAuthClientReadError, OAuthClientView};

#[async_trait::async_trait]
pub trait OAuthClientListReader: Send + Sync {
    async fn list(&self) -> Result<Vec<OAuthClientView>, OAuthClientReadError>;
}
