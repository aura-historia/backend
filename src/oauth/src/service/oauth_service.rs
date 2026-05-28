use crate::core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode,
};
use crate::core::client::{OAuthClient, OAuthClientId};
use crate::data::{IntrospectionResponseData, TokenResponseData, scope_string};
use crate::dynamodb::authorization_code_record::AuthorizationCodeRecord;
use crate::dynamodb::repository::OAuthRepository;
use aws_sdk_dynamodb::error::SdkError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::user_id::UserId;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use time::{Duration, OffsetDateTime};
use user::core::access_token::{AccessTokenOrigin, RawAccessToken, Scope};
use user::data::access_token_data::AccessTokenTypeData;
use user::service::command::CreateAccessTokenCommand;
use user::service::user_service::{UserService, UserServiceError};

const AUTHORIZATION_CODE_TTL: Duration = Duration::minutes(10);
const ACCESS_TOKEN_TTL: Duration = Duration::days(30);

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeRequest {
    pub response_type: String,
    pub client_id: OAuthClientId,
    pub redirect_uri: String,
    pub scope: HashSet<Scope>,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: CodeChallengeMethod,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeResponse {
    pub redirect_to: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: OAuthAuthorizationCode,
    pub redirect_uri: String,
    pub client_id: OAuthClientId,
    pub client_secret: RawAccessToken,
    pub code_verifier: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenIntrospectionRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawAccessToken,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenRevocationRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawAccessToken,
}

#[derive(thiserror::Error, Debug)]
pub enum OAuthServiceError {
    #[error("OAuth client not found.")]
    ClientNotFound,
    #[error("Invalid OAuth client secret.")]
    InvalidClientSecret,
    #[error("Unsupported response_type '{0}'.")]
    UnsupportedResponseType(String),
    #[error("Unsupported grant_type '{0}'.")]
    UnsupportedGrantType(String),
    #[error("Redirect URI is not registered for client.")]
    InvalidRedirectUri,
    #[error("Requested scope is not allowed for client.")]
    InvalidScope,
    #[error("Authorization code not found.")]
    AuthorizationCodeNotFound,
    #[error("Authorization code expired.")]
    AuthorizationCodeExpired,
    #[error("Authorization code does not belong to client.")]
    AuthorizationCodeClientMismatch,
    #[error("Authorization code redirect_uri mismatch.")]
    AuthorizationCodeRedirectUriMismatch,
    #[error("PKCE code_verifier did not match code_challenge.")]
    InvalidCodeVerifier,
    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(#[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),
    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(#[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError>),
    #[error("User service error: {0}")]
    UserServiceError(#[from] UserServiceError),
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait OAuthService {
    async fn authorize(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError>;

    async fn token(&self, request: TokenRequest) -> Result<TokenResponseData, OAuthServiceError>;

    async fn revoke(&self, request: TokenRevocationRequest) -> Result<(), OAuthServiceError>;

    async fn introspect(
        &self,
        request: TokenIntrospectionRequest,
    ) -> Result<IntrospectionResponseData, OAuthServiceError>;
}

pub struct OAuthServiceImpl<'a> {
    repository: &'a (dyn OAuthRepository + Sync),
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> OAuthServiceImpl<'a> {
    pub fn new(
        repository: &'a (dyn OAuthRepository + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            repository,
            user_service,
        }
    }

    async fn find_client(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClient, OAuthServiceError> {
        self.repository
            .get_client_record(client_id)
            .await?
            .map(Into::into)
            .ok_or(OAuthServiceError::ClientNotFound)
    }

    async fn authenticate_client(
        &self,
        client_id: &OAuthClientId,
        client_secret: &RawAccessToken,
    ) -> Result<OAuthClient, OAuthServiceError> {
        let client = self.find_client(client_id).await?;
        if client_secret.check(&client.hashed_client_secret) {
            Ok(client)
        } else {
            Err(OAuthServiceError::InvalidClientSecret)
        }
    }
}

#[async_trait::async_trait]
impl OAuthService for OAuthServiceImpl<'_> {
    async fn authorize(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError> {
        if request.response_type != "code" {
            return Err(OAuthServiceError::UnsupportedResponseType(
                request.response_type,
            ));
        }
        let client = self.find_client(&request.client_id).await?;
        if !client.redirect_uris.contains(&request.redirect_uri) {
            return Err(OAuthServiceError::InvalidRedirectUri);
        }
        if !request.scope.is_subset(&client.scopes) {
            return Err(OAuthServiceError::InvalidScope);
        }

        let now = OffsetDateTime::now_utc();
        let code = AuthorizationCode {
            code: OAuthAuthorizationCode::new(),
            client_id: request.client_id,
            user_id: *user_id,
            redirect_uri: request.redirect_uri.clone(),
            scopes: request.scope,
            code_challenge: request.code_challenge,
            code_challenge_method: request.code_challenge_method,
            expires: now + AUTHORIZATION_CODE_TTL,
            created: now,
        };
        self.repository
            .put_authorization_code_record(AuthorizationCodeRecord::from(code.clone()))
            .await?;

        let mut params = HashMap::from([("code", code.code.to_string())]);
        if let Some(state) = request.state {
            params.insert("state", state);
        }
        let redirect_to = append_query_params(&request.redirect_uri, params);
        Ok(AuthorizeResponse { redirect_to })
    }

    async fn token(&self, request: TokenRequest) -> Result<TokenResponseData, OAuthServiceError> {
        if request.grant_type != "authorization_code" {
            return Err(OAuthServiceError::UnsupportedGrantType(request.grant_type));
        }
        let client = self
            .authenticate_client(&request.client_id, &request.client_secret)
            .await?;
        let code = self
            .repository
            .get_authorization_code_record(&request.code)
            .await?
            .map(AuthorizationCode::from)
            .ok_or(OAuthServiceError::AuthorizationCodeNotFound)?;
        self.repository
            .delete_authorization_code_record(&request.code)
            .await?;
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

        let expires = OffsetDateTime::now_utc() + ACCESS_TOKEN_TTL;
        let (raw, access_token) = self
            .user_service
            .create_access_token(
                &code.user_id,
                CreateAccessTokenCommand {
                    name: format!("OAuth client {}", client.client_id).into(),
                    scopes: code.scopes,
                    expires: Some(expires),
                    origin: AccessTokenOrigin::OAuth {
                        client_id: client.client_id.to_string(),
                    },
                },
            )
            .await?;
        Ok(TokenResponseData {
            access_token: raw.into(),
            token_type: AccessTokenTypeData::Bearer,
            expires_in: access_token
                .expires
                .map(|expires| (expires - OffsetDateTime::now_utc()).whole_seconds().max(0)),
            scope: scope_string(&access_token.scopes),
        })
    }

    async fn revoke(&self, request: TokenRevocationRequest) -> Result<(), OAuthServiceError> {
        let _ = self
            .authenticate_client(&request.client_id, &request.client_secret)
            .await?;
        match self
            .user_service
            .delete_access_token_by_raw(&request.token)
            .await
        {
            Ok(()) | Err(UserServiceError::AccessTokenNotFoundByRaw) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn introspect(
        &self,
        request: TokenIntrospectionRequest,
    ) -> Result<IntrospectionResponseData, OAuthServiceError> {
        let _ = self
            .authenticate_client(&request.client_id, &request.client_secret)
            .await?;
        let token = match self
            .user_service
            .find_access_token_by_raw(&request.token)
            .await
        {
            Ok(token) => token,
            Err(UserServiceError::AccessTokenNotFoundByRaw) => {
                return Ok(IntrospectionResponseData {
                    active: false,
                    scope: None,
                    client_id: None,
                    sub: None,
                    token_type: None,
                    exp: None,
                    iat: None,
                });
            }
            Err(err) => return Err(err.into()),
        };
        let client_id = match &token.origin {
            AccessTokenOrigin::OAuth { client_id } => Some(client_id.clone()),
            AccessTokenOrigin::User => None,
        };
        Ok(IntrospectionResponseData {
            active: true,
            scope: Some(scope_string(&token.scopes)),
            client_id,
            sub: Some(token.user_id.to_string()),
            token_type: Some("Bearer".to_owned()),
            exp: token.expires.map(|expires| expires.unix_timestamp()),
            iat: Some(token.created.unix_timestamp()),
        })
    }
}

fn append_query_params(uri: &str, params: HashMap<&str, String>) -> String {
    let mut url = url::Url::parse(uri).expect("redirect_uri was validated as client URI");
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, &value);
    }
    url.to_string()
}

fn verify_s256(verifier: &str, expected_challenge: &str) -> bool {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == expected_challenge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_verify_s256_challenge() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_s256(verifier, challenge));
    }
}
