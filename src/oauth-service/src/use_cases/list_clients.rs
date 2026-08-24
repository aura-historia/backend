use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientListReader, OAuthClientView};

#[async_trait::async_trait]
pub trait ListOAuthClientsUseCase: Send + Sync {
    async fn execute(&self) -> Result<Vec<OAuthClientView>, OAuthServiceError>;
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
    R: OAuthClientListReader,
{
    async fn execute(&self) -> Result<Vec<OAuthClientView>, OAuthServiceError> {
        Ok(self.reader.list().await?)
    }
}
