use crate::error::OAuthServiceError;
use crate::ports::{OAuthAccessTokenGateway, OAuthAccessTokenGatewayError, OAuthClientRepository};
use crate::use_cases::support::authenticate_client;
use application::operation_context::OperationContext;
use common::oauth_client_id::OAuthClientId;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret};

#[derive(Debug, Clone, PartialEq)]
pub struct RevokeTokenRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
}

#[async_trait::async_trait]
pub trait RevokeTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError>;
}

pub struct RevokeTokenHandler<C, G> {
    clients: C,
    access_tokens: G,
}
impl<C, G> RevokeTokenHandler<C, G> {
    pub fn new(clients: C, access_tokens: G) -> Self {
        Self {
            clients,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<C, G> RevokeTokenUseCase for RevokeTokenHandler<C, G>
where
    C: OAuthClientRepository,
    G: OAuthAccessTokenGateway,
{
    async fn execute(
        &self,
        _context: &OperationContext,
        request: RevokeTokenRequest,
    ) -> Result<(), OAuthServiceError> {
        let _client =
            authenticate_client(&self.clients, &request.client_id, &request.client_secret).await?;
        match self.access_tokens.delete_raw(&request.token).await {
            Ok(()) | Err(OAuthAccessTokenGatewayError::NotFound) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
