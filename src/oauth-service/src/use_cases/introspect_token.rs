use crate::error::OAuthServiceError;
use crate::ports::{OAuthAccessTokenGateway, OAuthAccessTokenGatewayError, OAuthClientRepository};
use crate::use_cases::support::authenticate_client;
use crate::use_cases::token_by_authorization_code::OAuthTokenType;
use credential_core::oauth_client_id::OAuthClientId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenOrigin, RawAccessToken, RawOAuthClientSecret, Scope};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct IntrospectTokenRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntrospectTokenResponse {
    pub active: bool,
    pub scopes: Option<HashSet<Scope>>,
    pub client_id: Option<OAuthClientId>,
    pub subject: Option<UserId>,
    pub token_type: Option<OAuthTokenType>,
    pub expires: Option<OffsetDateTime>,
    pub issued_at: Option<OffsetDateTime>,
}

#[async_trait::async_trait]
pub trait IntrospectTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        request: IntrospectTokenRequest,
    ) -> Result<IntrospectTokenResponse, OAuthServiceError>;
}

pub struct IntrospectTokenHandler<C, G> {
    clients: C,
    access_tokens: G,
}
impl<C, G> IntrospectTokenHandler<C, G> {
    pub fn new(clients: C, access_tokens: G) -> Self {
        Self {
            clients,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<C, G> IntrospectTokenUseCase for IntrospectTokenHandler<C, G>
where
    C: OAuthClientRepository,
    G: OAuthAccessTokenGateway,
{
    async fn execute(
        &self,
        request: IntrospectTokenRequest,
    ) -> Result<IntrospectTokenResponse, OAuthServiceError> {
        let _client =
            authenticate_client(&self.clients, &request.client_id, &request.client_secret).await?;
        let token = match self.access_tokens.find_raw(&request.token).await {
            Ok(token) => token,
            Err(OAuthAccessTokenGatewayError::NotFound | OAuthAccessTokenGatewayError::Expired) => {
                return Ok(inactive());
            }
            Err(err) => return Err(err.into()),
        };
        let client_id = match token.origin {
            AccessTokenOrigin::OAuth { client_id } => Some(client_id),
            AccessTokenOrigin::User => None,
        };
        Ok(IntrospectTokenResponse {
            active: true,
            scopes: Some(token.scopes),
            client_id,
            subject: Some(token.user_id),
            token_type: Some(OAuthTokenType::Bearer),
            expires: token.expires,
            issued_at: token.issued_at,
        })
    }
}

fn inactive() -> IntrospectTokenResponse {
    IntrospectTokenResponse {
        active: false,
        scopes: None,
        client_id: None,
        subject: None,
        token_type: None,
        expires: None,
        issued_at: None,
    }
}
