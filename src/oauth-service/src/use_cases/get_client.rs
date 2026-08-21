use crate::error::OAuthServiceError;
use crate::ports::OAuthClientRepository;
use crate::use_cases::support::find_client;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::client::OAuthClient;

#[async_trait::async_trait]
pub trait GetOAuthClientUseCase: Send + Sync {
    async fn execute(&self, client_id: &OAuthClientId) -> Result<OAuthClient, OAuthServiceError>;
}

pub struct GetOAuthClientHandler<R> {
    reader: R,
}
impl<R> GetOAuthClientHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}
#[async_trait::async_trait]
impl<R> GetOAuthClientUseCase for GetOAuthClientHandler<R>
where
    R: OAuthClientRepository,
{
    async fn execute(&self, client_id: &OAuthClientId) -> Result<OAuthClient, OAuthServiceError> {
        find_client(&self.reader, client_id).await
    }
}
