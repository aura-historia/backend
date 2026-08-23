use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, NewOAuthAccessToken, OAuthAccessTokenGateway,
    OAuthClientRepository, ThirdPartyExchangeCodeRepository,
};
use crate::use_cases::support::{THIRD_PARTY_EXCHANGE_CODE_TTL, authenticate_client, verify_s256};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{OAuthAuthorizationCode, OAuthCodeVerifier};
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{RawAccessToken, RawOAuthClientSecret, Scope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthGrantType {
    AuthorizationCode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthTokenType {
    Bearer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenByAuthorizationCodeRequest {
    pub grant_type: OAuthGrantType,
    pub code: OAuthAuthorizationCode,
    pub redirect_uri: url::Url,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
    pub code_verifier: OAuthCodeVerifier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenResponse {
    pub access_token: RawAccessToken,
    pub token_type: OAuthTokenType,
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub third_party_exchange_code: Option<ThirdPartyExchangeCode>,
}

#[async_trait::async_trait]
pub trait TokenByAuthorizationCodeUseCase: Send + Sync {
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

pub struct TokenByAuthorizationCodeHandler<C, A, T, G> {
    clients: C,
    codes: A,
    exchange_codes: T,
    access_tokens: G,
}
impl<C, A, T, G> TokenByAuthorizationCodeHandler<C, A, T, G> {
    pub fn new(clients: C, codes: A, exchange_codes: T, access_tokens: G) -> Self {
        Self {
            clients,
            codes,
            exchange_codes,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<C, A, T, G> TokenByAuthorizationCodeUseCase for TokenByAuthorizationCodeHandler<C, A, T, G>
where
    C: OAuthClientRepository,
    A: AuthorizationCodeRepository,
    T: ThirdPartyExchangeCodeRepository,
    G: OAuthAccessTokenGateway,
{
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let client =
            authenticate_client(&self.clients, &request.client_id, &request.client_secret).await?;
        let code = self
            .codes
            .find_by_code(&request.code)
            .await?
            .ok_or(OAuthServiceError::AuthorizationCodeNotFound)?;
        self.codes.delete(&request.code).await?;
        if code.is_expired() {
            return Err(OAuthServiceError::AuthorizationCodeExpired);
        }
        if code.client_id != request.client_id {
            return Err(OAuthServiceError::AuthorizationCodeClientMismatch);
        }
        if code.redirect_uri != request.redirect_uri {
            return Err(OAuthServiceError::AuthorizationCodeRedirectUriMismatch);
        }
        if !verify_s256(&request.code_verifier, &code.code_challenge) {
            return Err(OAuthServiceError::InvalidCodeVerifier);
        }
        let issued = self
            .access_tokens
            .issue(NewOAuthAccessToken {
                user_id: code.user_id,
                client_id: client.client_id,
                client_name: client.name.to_string(),
                scopes: code.scopes,
            })
            .await?;
        let now = OffsetDateTime::now_utc();
        let third_party_exchange_code = ThirdPartyExchangeCodeGrant {
            code: ThirdPartyExchangeCode::new(),
            access_token: issued.raw.clone(),
            access_token_expires: issued.expires,
            scopes: issued.scopes.clone(),
            expires: now + THIRD_PARTY_EXCHANGE_CODE_TTL,
            created: now,
        };
        self.exchange_codes
            .insert(third_party_exchange_code.clone())
            .await?;
        Ok(TokenResponse {
            access_token: issued.raw,
            token_type: OAuthTokenType::Bearer,
            expires: issued.expires,
            scopes: issued.scopes,
            third_party_exchange_code: Some(third_party_exchange_code.code),
        })
    }
}
