use crate::error::OAuthServiceError;
use crate::ports::OAuthClientRepository;
use crate::use_cases::support::authorize_oauth_admin;
use application::operation_context::OperationContext;
use credential_core::oauth_client_id::OAuthClientId;

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteOAuthClientResult {
    pub client_id: OAuthClientId,
}

#[async_trait::async_trait]
pub trait DeleteOAuthClientUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError>;
}

pub struct DeleteOAuthClientHandler<R> {
    repository: R,
}
impl<R> DeleteOAuthClientHandler<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }
}
#[async_trait::async_trait]
impl<R> DeleteOAuthClientUseCase for DeleteOAuthClientHandler<R>
where
    R: OAuthClientRepository,
{
    async fn execute(
        &self,
        context: &OperationContext,
        client_id: &OAuthClientId,
    ) -> Result<DeleteOAuthClientResult, OAuthServiceError> {
        authorize_oauth_admin(context)?;
        self.repository.delete(client_id).await?;
        tracing::info!(event = "oauth_client.deleted", actor_id = %context.principal.label(), client_id = %client_id, outcome = "success");
        Ok(DeleteOAuthClientResult {
            client_id: *client_id,
        })
    }
}
