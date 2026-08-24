use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientDetailsReader, OAuthClientView};
use crate::use_cases::support::authorize_oauth_client_read;
use application::operation_context::OperationContext;
use credential_core::oauth_client_id::OAuthClientId;

#[async_trait::async_trait]
pub trait GetOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError>;
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
    R: OAuthClientDetailsReader,
{
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClientView, OAuthServiceError> {
        authorize_oauth_client_read(context)?;
        self.reader
            .find(client_id)
            .await?
            .ok_or(OAuthServiceError::ClientNotFound)
    }
}
