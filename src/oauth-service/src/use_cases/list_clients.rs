use crate::error::OAuthServiceError;
use crate::ports::OAuthClientReader;
use oauth_core::client::OAuthClient;

#[async_trait::async_trait]
pub trait ListOAuthClientsUseCase: Send + Sync {
    async fn execute(&self) -> Result<Vec<OAuthClient>, OAuthServiceError>;
}

pub struct ListOAuthClientsHandler<R> {
    reader: R,
}
impl<R> ListOAuthClientsHandler<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
}
#[async_trait::async_trait]
impl<R> ListOAuthClientsUseCase for ListOAuthClientsHandler<R>
where
    R: OAuthClientReader,
{
    async fn execute(&self) -> Result<Vec<OAuthClient>, OAuthServiceError> {
        Ok(self.reader.list().await?)
    }
}
