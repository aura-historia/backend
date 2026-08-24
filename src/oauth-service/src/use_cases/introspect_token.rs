use crate::error::OAuthServiceError;
use crate::ports::OAuthClientAuthenticationReader;
use crate::use_cases::support::authenticate_client_reader;
use crate::use_cases::token_by_authorization_code::OAuthTokenType;
use credential_core::oauth_client_id::OAuthClientId;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessTokenOrigin, HashedRawAccessToken, RawAccessToken, RawOAuthClientSecret, Scope,
};
use user_core::user_id::UserId;
use user_service::ports::AccessTokenAuthenticationReader;

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

pub struct IntrospectTokenHandler<C, R> {
    clients: C,
    access_tokens: R,
}
impl<C, R> IntrospectTokenHandler<C, R> {
    pub fn new(clients: C, access_tokens: R) -> Self {
        Self {
            clients,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<C, R> IntrospectTokenUseCase for IntrospectTokenHandler<C, R>
where
    C: OAuthClientAuthenticationReader,
    R: AccessTokenAuthenticationReader,
{
    async fn execute(
        &self,
        request: IntrospectTokenRequest,
    ) -> Result<IntrospectTokenResponse, OAuthServiceError> {
        authenticate_client_reader(&self.clients, &request.client_id, &request.client_secret)
            .await?;

        let now = OffsetDateTime::now_utc();
        let hashed_token = HashedRawAccessToken::from(request.token);
        let response = match self
            .access_tokens
            .find_authentication_by_hashed_token(&hashed_token)
            .await?
        {
            None => inactive(),
            Some(token) if token.expires.is_some_and(|expires| expires < now) => inactive(),
            Some(token) => {
                let client_id = match token.origin {
                    AccessTokenOrigin::OAuth { client_id } => Some(client_id),
                    AccessTokenOrigin::User => None,
                };
                IntrospectTokenResponse {
                    active: true,
                    scopes: Some(token.scopes),
                    client_id,
                    subject: Some(token.user_id),
                    token_type: Some(OAuthTokenType::Bearer),
                    expires: token.expires,
                    issued_at: None,
                }
            }
        };
        Ok(response)
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
