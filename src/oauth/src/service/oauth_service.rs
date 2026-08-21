use crate::core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    OAuthCodeVerifier,
};
use crate::core::client::{OAuthClient, OAuthClientName};
use crate::core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use crate::dynamodb::authorization_code_record::AuthorizationCodeRecord;
use crate::dynamodb::client_record::OAuthClientRecord;
use crate::dynamodb::client_record_update::OAuthClientRecordUpdate;
use crate::dynamodb::repository::OAuthRepository;
use crate::dynamodb::third_party_exchange_code_record::ThirdPartyExchangeCodeRecord;
use aws_sdk_dynamodb::error::SdkError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::api::error_code::{
    OAUTH_AUTHORIZATION_CODE_CLIENT_MISMATCH, OAUTH_AUTHORIZATION_CODE_EXPIRED,
    OAUTH_AUTHORIZATION_CODE_NOT_FOUND, OAUTH_AUTHORIZATION_REDIRECT_URI_MISMATCH,
    OAUTH_CLIENT_FORBIDDEN, OAUTH_CLIENT_NOT_FOUND, OAUTH_INVALID_CLIENT_METADATA,
    OAUTH_INVALID_CLIENT_SECRET, OAUTH_INVALID_CODE_VERIFIER, OAUTH_INVALID_REDIRECT_URI,
    OAUTH_INVALID_SCOPE, OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND,
};
use common::oauth_client_id::OAuthClientId;
use common::{
    actor::{RequestContext, domain::Actor},
    string_newtype,
    user_id::UserId,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use time::OffsetDateTime;
use tracing::info;
use url::Url;
use user::core::access_token::{
    AccessTokenOrigin, HashedRawOAuthClientSecret, RawAccessToken, RawOAuthClientSecret, Scope,
};
use user::service::command::CreateAccessTokenCommand;
use user::service::user_service::{UserService, UserServiceError};

const AUTHORIZATION_CODE_TTL: time::Duration = time::Duration::minutes(10);
const THIRD_PARTY_EXCHANGE_CODE_TTL: time::Duration = time::Duration::seconds(60);

string_newtype!(OAuthState);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthResponseType {
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthGrantType {
    AuthorizationCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthTokenType {
    Bearer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeRequest {
    pub response_type: OAuthResponseType,
    pub client_id: OAuthClientId,
    pub redirect_uri: url::Url,
    pub scope: HashSet<Scope>,
    pub state: Option<OAuthState>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizeResponse {
    pub redirect_to: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenRequest {
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

#[derive(Debug, Clone, PartialEq)]
pub struct TokenIntrospectionRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IntrospectionResponse {
    pub active: bool,
    pub scopes: Option<HashSet<Scope>>,
    pub client_id: Option<OAuthClientId>,
    pub subject: Option<UserId>,
    pub token_type: Option<OAuthTokenType>,
    pub expires: Option<OffsetDateTime>,
    pub issued_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenRevocationRequest {
    pub token: RawAccessToken,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOAuthClientCommand {
    pub name: OAuthClientName,
    pub redirect_uris: HashSet<url::Url>,
    pub tos_uri: Url,
    pub policy_uri: Url,
    pub client_uri: Url,
    pub logo_uri: Url,
    pub scopes: HashSet<Scope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOAuthClientCommand {
    pub name: Option<OAuthClientName>,
    pub redirect_uris: Option<HashSet<url::Url>>,
    pub tos_uri: Option<Url>,
    pub policy_uri: Option<Url>,
    pub client_uri: Option<Url>,
    pub logo_uri: Option<Url>,
    pub scopes: Option<HashSet<Scope>>,
}

#[derive(thiserror::Error, Debug)]
pub enum OAuthServiceError {
    #[error("OAuth client not found.")]
    ClientNotFound,
    #[error("Invalid OAuth client secret.")]
    InvalidClientSecret,
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
    #[error("Third-party exchange code not found.")]
    ThirdPartyExchangeCodeNotFound,
    #[error("Third-party exchange code expired.")]
    ThirdPartyExchangeCodeExpired,
    #[error("OAuth client does not belong to the authenticated user.")]
    ClientForbidden,
    #[error("OAuth client metadata is invalid: {0}")]
    InvalidClientMetadata(String),
    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),
    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(#[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),
    #[error("Encountered DynamoDB SdkError for Query: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError>),
    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(#[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError>),
    #[error("Encountered DynamoDB SdkError for DeleteItem: {0:?}")]
    SdkDeleteItemError(#[from] SdkError<aws_sdk_dynamodb::operation::delete_item::DeleteItemError>),
    #[error("User service error: {0}")]
    UserServiceError(#[from] UserServiceError),
}

impl From<OAuthServiceError> for common::api::error::ApiError {
    fn from(err: OAuthServiceError) -> Self {
        match err {
            OAuthServiceError::InvalidClientSecret => {
                common::api::error::ApiError::unauthorized(OAUTH_INVALID_CLIENT_SECRET)
                    .with_detail(err.to_string())
            }
            OAuthServiceError::ClientNotFound => {
                common::api::error::ApiError::unauthorized(OAUTH_CLIENT_NOT_FOUND)
                    .with_detail(err.to_string())
            }
            OAuthServiceError::InvalidRedirectUri => {
                common::api::error::ApiError::bad_request(OAUTH_INVALID_REDIRECT_URI, Box::new(err))
            }
            OAuthServiceError::InvalidScope => {
                common::api::error::ApiError::bad_request(OAUTH_INVALID_SCOPE, Box::new(err))
            }
            OAuthServiceError::ClientForbidden => {
                common::api::error::ApiError::forbidden(OAUTH_CLIENT_FORBIDDEN)
                    .with_detail(err.to_string())
            }
            OAuthServiceError::InvalidClientMetadata(_) => {
                common::api::error::ApiError::bad_request(
                    OAUTH_INVALID_CLIENT_METADATA,
                    Box::new(err),
                )
            }
            OAuthServiceError::ThirdPartyExchangeCodeNotFound
            | OAuthServiceError::ThirdPartyExchangeCodeExpired => {
                common::api::error::ApiError::bad_request(
                    OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND,
                    Box::new(err),
                )
            }
            OAuthServiceError::AuthorizationCodeNotFound => {
                common::api::error::ApiError::bad_request(
                    OAUTH_AUTHORIZATION_CODE_NOT_FOUND,
                    Box::new(err),
                )
            }
            OAuthServiceError::AuthorizationCodeExpired => {
                common::api::error::ApiError::bad_request(
                    OAUTH_AUTHORIZATION_CODE_EXPIRED,
                    Box::new(err),
                )
            }
            OAuthServiceError::AuthorizationCodeClientMismatch => {
                common::api::error::ApiError::bad_request(
                    OAUTH_AUTHORIZATION_CODE_CLIENT_MISMATCH,
                    Box::new(err),
                )
            }
            OAuthServiceError::AuthorizationCodeRedirectUriMismatch => {
                common::api::error::ApiError::bad_request(
                    OAUTH_AUTHORIZATION_REDIRECT_URI_MISMATCH,
                    Box::new(err),
                )
            }
            OAuthServiceError::InvalidCodeVerifier => common::api::error::ApiError::bad_request(
                OAUTH_INVALID_CODE_VERIFIER,
                Box::new(err),
            ),
            OAuthServiceError::SdkGetItemError(sdk_error) => sdk_error.into(),
            OAuthServiceError::SdkPutItemError(sdk_error) => sdk_error.into(),
            OAuthServiceError::SdkQueryError(sdk_error) => sdk_error.into(),
            OAuthServiceError::SdkUpdateItemError(sdk_error) => sdk_error.into(),
            OAuthServiceError::SdkDeleteItemError(sdk_error) => sdk_error.into(),
            OAuthServiceError::UserServiceError(user_err) => user_err.into(),
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait OAuthService {
    async fn create_client(
        &self,
        ctx: &RequestContext,
        command: CreateOAuthClientCommand,
    ) -> Result<(RawOAuthClientSecret, OAuthClient), OAuthServiceError>;

    async fn get_clients(&self) -> Result<Vec<OAuthClient>, OAuthServiceError>;

    async fn get_client(&self, client_id: &OAuthClientId)
    -> Result<OAuthClient, OAuthServiceError>;

    async fn update_client(
        &self,
        ctx: &RequestContext,
        client_id: &OAuthClientId,
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClient, OAuthServiceError>;

    async fn delete_client(
        &self,
        ctx: &RequestContext,
        client_id: &OAuthClientId,
    ) -> Result<(), OAuthServiceError>;

    async fn authorize(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError>;

    async fn token(&self, request: TokenRequest) -> Result<TokenResponse, OAuthServiceError>;

    async fn token_by_third_party_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError>;

    async fn revoke(
        &self,
        ctx: &RequestContext,
        request: TokenRevocationRequest,
    ) -> Result<(), OAuthServiceError>;

    async fn introspect(
        &self,
        request: TokenIntrospectionRequest,
    ) -> Result<IntrospectionResponse, OAuthServiceError>;
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

    // Keep the legacy DynamoDB error-by-value API stable until OAuth is retired.
    #[allow(clippy::result_large_err)]
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

    // Keep the legacy DynamoDB error-by-value API stable until OAuth is retired.
    #[allow(clippy::result_large_err)]
    async fn authenticate_client(
        &self,
        client_id: &OAuthClientId,
        client_secret: &RawOAuthClientSecret,
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
    async fn create_client(
        &self,
        ctx: &RequestContext,
        command: CreateOAuthClientCommand,
    ) -> Result<(RawOAuthClientSecret, OAuthClient), OAuthServiceError> {
        validate_redirect_uris(&command.redirect_uris)
            .map_err(OAuthServiceError::InvalidClientMetadata)?;
        let now = OffsetDateTime::now_utc();
        let raw_secret = RawOAuthClientSecret::new();
        let client = OAuthClient {
            client_id: OAuthClientId::new(),
            hashed_client_secret: HashedRawOAuthClientSecret::from(raw_secret.clone()),
            name: command.name,
            tos_uri: command.tos_uri,
            policy_uri: command.policy_uri,
            client_uri: command.client_uri,
            logo_uri: command.logo_uri,
            redirect_uris: command.redirect_uris,
            scopes: command.scopes,
            created_by: ctx.actor,
            updated_by: ctx.actor,
            created: now,
            updated: now,
        };
        self.repository
            .put_client_record(OAuthClientRecord::from((
                client.clone(),
                raw_secret.clone(),
            )))
            .await?;
        info!(
            actor = %ctx.actor,
            clientId = %client.client_id,
            "Created OAuth client."
        );
        Ok((raw_secret, client))
    }

    async fn get_clients(&self) -> Result<Vec<OAuthClient>, OAuthServiceError> {
        Ok(self
            .repository
            .query_client_records()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn get_client(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<OAuthClient, OAuthServiceError> {
        self.find_client(client_id).await
    }

    async fn update_client(
        &self,
        ctx: &RequestContext,
        client_id: &OAuthClientId,
        command: UpdateOAuthClientCommand,
    ) -> Result<OAuthClient, OAuthServiceError> {
        if let Some(redirect_uris) = &command.redirect_uris {
            validate_redirect_uris(redirect_uris)
                .map_err(OAuthServiceError::InvalidClientMetadata)?;
        }
        let update = OAuthClientRecordUpdate {
            name: command.name.clone(),
            tos_uri: command.tos_uri.clone(),
            policy_uri: command.policy_uri.clone(),
            client_uri: command.client_uri.clone(),
            logo_uri: command.logo_uri.clone(),
            redirect_uris: command.redirect_uris.clone(),
            scopes: command
                .scopes
                .clone()
                .map(|scopes| scopes.into_iter().map(Into::into).collect()),
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };
        info!(
            actor = %ctx.actor,
            clientId = %client_id,
            update = ?command,
            "Updated OAuth client."
        );
        self.repository
            .update_client_record(client_id, update)
            .await?
            .map(Into::into)
            .ok_or(OAuthServiceError::ClientNotFound)
    }

    async fn delete_client(
        &self,
        ctx: &RequestContext,
        client_id: &OAuthClientId,
    ) -> Result<(), OAuthServiceError> {
        self.repository.delete_client_record(client_id).await?;
        info!(
            actor = %ctx.actor,
            clientId = %client_id,
            "Deleted OAuth client."
        );
        Ok(())
    }

    async fn authorize(
        &self,
        user_id: &UserId,
        request: AuthorizeRequest,
    ) -> Result<AuthorizeResponse, OAuthServiceError> {
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
            params.insert("state", state.as_ref().to_owned());
        }
        let redirect_to = append_query_params(&request.redirect_uri, params);
        Ok(AuthorizeResponse { redirect_to })
    }

    async fn token(&self, request: TokenRequest) -> Result<TokenResponse, OAuthServiceError> {
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

        let (raw, access_token) = self
            .user_service
            .create_access_token(
                &RequestContext {
                    actor: Actor::User(code.user_id),
                },
                &code.user_id,
                CreateAccessTokenCommand {
                    name: format!("{} (OAuth-Client {})", client.name, client.client_id).into(),
                    scopes: code.scopes,
                    expires: None,
                    origin: AccessTokenOrigin::OAuth {
                        client_id: client.client_id,
                    },
                },
            )
            .await?;
        let now = OffsetDateTime::now_utc();
        let third_party_exchange_code = ThirdPartyExchangeCodeGrant {
            code: ThirdPartyExchangeCode::new(),
            access_token: raw.clone(),
            access_token_expires: access_token.expires,
            scopes: access_token.scopes.clone(),
            expires: now + THIRD_PARTY_EXCHANGE_CODE_TTL,
            created: now,
        };
        self.repository
            .put_third_party_exchange_code_record(ThirdPartyExchangeCodeRecord::from(
                third_party_exchange_code.clone(),
            ))
            .await?;
        Ok(TokenResponse {
            access_token: raw,
            token_type: OAuthTokenType::Bearer,
            expires: access_token.expires,
            scopes: access_token.scopes,
            third_party_exchange_code: Some(third_party_exchange_code.code),
        })
    }

    async fn token_by_third_party_code(
        &self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let exchange_code = self
            .repository
            .get_third_party_exchange_code_record(code)
            .await?
            .map(ThirdPartyExchangeCodeGrant::from)
            .ok_or(OAuthServiceError::ThirdPartyExchangeCodeNotFound)?;
        self.repository
            .delete_third_party_exchange_code_record(code)
            .await?;
        if exchange_code.is_expired() {
            return Err(OAuthServiceError::ThirdPartyExchangeCodeExpired);
        }

        Ok(TokenResponse {
            access_token: exchange_code.access_token,
            token_type: OAuthTokenType::Bearer,
            expires: exchange_code.access_token_expires,
            scopes: exchange_code.scopes,
            third_party_exchange_code: None,
        })
    }

    async fn revoke(
        &self,
        ctx: &RequestContext,
        request: TokenRevocationRequest,
    ) -> Result<(), OAuthServiceError> {
        let _ = self
            .authenticate_client(&request.client_id, &request.client_secret)
            .await?;
        match self
            .user_service
            .delete_access_token_by_raw(ctx, &request.token)
            .await
        {
            Ok(()) | Err(UserServiceError::AccessTokenNotFoundByRaw) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn introspect(
        &self,
        request: TokenIntrospectionRequest,
    ) -> Result<IntrospectionResponse, OAuthServiceError> {
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
                return Ok(IntrospectionResponse {
                    active: false,
                    scopes: None,
                    client_id: None,
                    subject: None,
                    token_type: None,
                    expires: None,
                    issued_at: None,
                });
            }
            Err(err) => return Err(err.into()),
        };
        let client_id = match &token.origin {
            AccessTokenOrigin::OAuth { client_id } => Some(*client_id),
            AccessTokenOrigin::User => None,
        };
        Ok(IntrospectionResponse {
            active: true,
            scopes: Some(token.scopes),
            client_id,
            subject: Some(token.user_id),
            token_type: Some(OAuthTokenType::Bearer),
            expires: token.expires,
            issued_at: Some(token.created),
        })
    }
}

fn append_query_params(uri: &url::Url, params: HashMap<&str, String>) -> String {
    let mut url = uri.clone();
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, &value);
    }
    url.to_string()
}

fn verify_s256(verifier: &str, expected_challenge: &str) -> bool {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == expected_challenge
}

fn validate_redirect_uris(redirect_uris: &HashSet<url::Url>) -> Result<(), String> {
    if redirect_uris.is_empty() {
        return Err("redirect_uris cannot be empty".to_owned());
    }
    for redirect_uri in redirect_uris {
        if redirect_uri.scheme() != "https" {
            return Err(format!("redirect_uri '{redirect_uri}' must use https"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::client::OAuthClientName;
    use crate::dynamodb::client_record::OAuthClientRecord;
    use crate::dynamodb::repository::MockOAuthRepository;
    use user::core::access_token::{
        AccessToken, AccessTokenId, AccessTokenOrigin, RawOAuthClientSecret,
    };
    use user::service::user_service::MockUserService;

    fn client_id() -> OAuthClientId {
        OAuthClientId::try_from("018f6e7a-8b9c-7d0e-8f12-3456789abcde").unwrap()
    }

    fn oauth_client(secret: &RawOAuthClientSecret) -> OAuthClient {
        let now = OffsetDateTime::now_utc();
        OAuthClient {
            client_id: client_id(),
            hashed_client_secret: secret.clone().into(),
            name: OAuthClientName::from("Client"),
            redirect_uris: HashSet::from([
                url::Url::parse("https://client.example/callback").unwrap()
            ]),
            tos_uri: Url::parse("https://client.example/tos").unwrap(),
            policy_uri: Url::parse("https://client.example/policy").unwrap(),
            client_uri: Url::parse("https://client.example").unwrap(),
            logo_uri: Url::parse("https://client.example/logo.png").unwrap(),
            scopes: HashSet::from([Scope::ProductsWrite]),
            created_by: Actor::User(UserId::new()),
            updated_by: Actor::System,
            created: now,
            updated: now,
        }
    }

    fn create_command(redirect_uri: &str) -> CreateOAuthClientCommand {
        CreateOAuthClientCommand {
            name: OAuthClientName::from("Client"),
            redirect_uris: HashSet::from([url::Url::parse(redirect_uri).unwrap()]),
            tos_uri: Url::parse("https://client.example/tos").unwrap(),
            policy_uri: Url::parse("https://client.example/policy").unwrap(),
            client_uri: Url::parse("https://client.example").unwrap(),
            logo_uri: Url::parse("https://client.example/logo.png").unwrap(),
            scopes: HashSet::from([Scope::ProductsWrite]),
        }
    }

    #[test]
    fn should_verify_s256_challenge() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verify_s256(verifier, challenge));
    }

    #[tokio::test]
    async fn should_create_authorization_code() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let client_record = OAuthClientRecord::from((client.clone(), secret.clone()));
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_client_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(client_record)) }));
        repository
            .expect_put_authorization_code_record()
            .return_once(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::put_item::PutItemOutput::builder().build())
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let response = service
            .authorize(
                &UserId::new(),
                AuthorizeRequest {
                    response_type: OAuthResponseType::Code,
                    client_id: client.client_id,
                    redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
                    scope: HashSet::from([Scope::ProductsWrite]),
                    state: Some(OAuthState::from("state_1")),
                    code_challenge: OAuthCodeChallenge::from("challenge"),
                    code_challenge_method: CodeChallengeMethod::S256,
                },
            )
            .await
            .unwrap();

        assert!(
            response
                .redirect_to
                .starts_with("https://client.example/callback?")
        );
        assert!(response.redirect_to.contains("code="));
        assert!(response.redirect_to.contains("state=state_1"));
    }

    #[tokio::test]
    async fn should_reject_invalid_client_secret() {
        let secret = RawOAuthClientSecret::new();
        let client_record = OAuthClientRecord::from((oauth_client(&secret), secret.clone()));
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_client_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(client_record)) }));
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .introspect(TokenIntrospectionRequest {
                token: RawAccessToken::new(),
                client_id: client_id(),
                client_secret: RawOAuthClientSecret::new(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::InvalidClientSecret));
    }

    #[tokio::test]
    async fn should_create_oauth_client() {
        let user_id = UserId::new();
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_put_client_record()
            .return_once(move |record| {
                assert_eq!(
                    common::actor::record::ActorRecord::User(user_id),
                    record.created_by
                );
                assert_eq!(OAuthClientName::from("Client"), record.name);
                assert_eq!(
                    Url::parse("https://client.example/tos").unwrap(),
                    record.tos_uri
                );
                assert_eq!(
                    Url::parse("https://client.example/policy").unwrap(),
                    record.policy_uri
                );
                assert_eq!(
                    Url::parse("https://client.example").unwrap(),
                    record.client_uri
                );
                assert_eq!(
                    Url::parse("https://client.example/logo.png").unwrap(),
                    record.logo_uri
                );
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::put_item::PutItemOutput::builder().build())
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let (secret, client) = service
            .create_client(
                &RequestContext {
                    actor: Actor::User(user_id),
                },
                create_command("https://client.example/callback"),
            )
            .await
            .unwrap();

        assert!(secret.check(&client.hashed_client_secret));
        assert_eq!(
            common::actor::domain::Actor::User(user_id),
            client.created_by
        );
        assert_eq!(
            Url::parse("https://client.example/tos").unwrap(),
            client.tos_uri
        );
        assert_eq!(
            Url::parse("https://client.example/policy").unwrap(),
            client.policy_uri
        );
        assert_eq!(
            Url::parse("https://client.example").unwrap(),
            client.client_uri
        );
        assert_eq!(
            Url::parse("https://client.example/logo.png").unwrap(),
            client.logo_uri
        );
    }

    #[tokio::test]
    async fn should_reject_invalid_oauth_client_metadata() {
        let repository = MockOAuthRepository::default();
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .create_client(
                &RequestContext {
                    actor: Actor::System,
                },
                create_command("http://client.example/callback"),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::InvalidClientMetadata(_)));
    }

    #[tokio::test]
    async fn should_get_oauth_clients() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_query_client_records()
            .return_once(move || {
                Box::pin(async move { Ok(vec![OAuthClientRecord::from((client, secret))]) })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let clients = service.get_clients().await.unwrap();

        assert_eq!(1, clients.len());
    }

    #[tokio::test]
    async fn should_reject_missing_oauth_client() {
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_client_record()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service.get_client(&client_id()).await.unwrap_err();

        assert!(matches!(err, OAuthServiceError::ClientNotFound));
    }

    #[tokio::test]
    async fn should_update_oauth_client() {
        let secret = RawOAuthClientSecret::new();
        let mut client = oauth_client(&secret);
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_update_client_record()
            .return_once(move |actual_client_id, update| {
                assert_eq!(&client.client_id, actual_client_id);
                client.name = update.name.unwrap();
                client.tos_uri = update.tos_uri.unwrap();
                Box::pin(async move { Ok(Some(OAuthClientRecord::from((client, secret)))) })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let updated = service
            .update_client(
                &RequestContext {
                    actor: Actor::System,
                },
                &client_id(),
                UpdateOAuthClientCommand {
                    name: Some(OAuthClientName::from("Updated")),
                    tos_uri: Some(Url::parse("https://client.example/updated-tos").unwrap()),
                    policy_uri: None,
                    client_uri: None,
                    logo_uri: None,
                    redirect_uris: None,
                    scopes: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(OAuthClientName::from("Updated"), updated.name);
        assert_eq!(
            Url::parse("https://client.example/updated-tos").unwrap(),
            updated.tos_uri
        );
    }

    #[tokio::test]
    async fn should_reject_missing_oauth_client_update() {
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_update_client_record()
            .return_once(|_, _| Box::pin(async { Ok(None) }));
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .update_client(
                &RequestContext {
                    actor: Actor::System,
                },
                &client_id(),
                UpdateOAuthClientCommand {
                    name: Some(OAuthClientName::from("Updated")),
                    tos_uri: None,
                    policy_uri: None,
                    client_uri: None,
                    logo_uri: None,
                    redirect_uris: None,
                    scopes: None,
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::ClientNotFound));
    }

    #[tokio::test]
    async fn should_delete_oauth_client() {
        let mut repository = MockOAuthRepository::default();
        repository.expect_delete_client_record().return_once(|_| {
            Box::pin(async {
                Ok(aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder().build())
            })
        });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        service
            .delete_client(
                &RequestContext {
                    actor: Actor::System,
                },
                &client_id(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_reject_authorize_with_invalid_redirect_uri() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let mut repository = MockOAuthRepository::default();
        repository.expect_get_client_record().return_once(move |_| {
            Box::pin(async move { Ok(Some(OAuthClientRecord::from((client, secret)))) })
        });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .authorize(
                &UserId::new(),
                AuthorizeRequest {
                    response_type: OAuthResponseType::Code,
                    client_id: client_id(),
                    redirect_uri: url::Url::parse("https://client.example/other").unwrap(),
                    scope: HashSet::from([Scope::ProductsWrite]),
                    state: None,
                    code_challenge: OAuthCodeChallenge::from("challenge"),
                    code_challenge_method: CodeChallengeMethod::S256,
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::InvalidRedirectUri));
    }

    #[tokio::test]
    async fn should_reject_authorize_with_invalid_scope() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let mut repository = MockOAuthRepository::default();
        repository.expect_get_client_record().return_once(move |_| {
            Box::pin(async move { Ok(Some(OAuthClientRecord::from((client, secret)))) })
        });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .authorize(
                &UserId::new(),
                AuthorizeRequest {
                    response_type: OAuthResponseType::Code,
                    client_id: client_id(),
                    redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
                    scope: HashSet::from([Scope::ShopsManage]),
                    state: None,
                    code_challenge: OAuthCodeChallenge::from("challenge"),
                    code_challenge_method: CodeChallengeMethod::S256,
                },
            )
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::InvalidScope));
    }

    #[tokio::test]
    async fn should_exchange_authorization_code_for_access_token() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let now = OffsetDateTime::now_utc();
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let code = AuthorizationCode {
            code: OAuthAuthorizationCode::new(),
            client_id: client.client_id,
            user_id: UserId::new(),
            redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
            scopes: HashSet::from([Scope::ProductsWrite]),
            code_challenge: OAuthCodeChallenge::from("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
            code_challenge_method: CodeChallengeMethod::S256,
            expires: now + time::Duration::minutes(10),
            created: now,
        };
        let code_record = AuthorizationCodeRecord::from(code.clone());
        let client_record = OAuthClientRecord::from((client.clone(), secret.clone()));
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_client_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(client_record)) }));
        repository
            .expect_get_authorization_code_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(code_record)) }));
        repository
            .expect_delete_authorization_code_record()
            .return_once(|_| {
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder()
                            .build(),
                    )
                })
            });
        repository
            .expect_put_third_party_exchange_code_record()
            .return_once(|record| {
                assert_eq!(
                    HashSet::from([
                        user::dynamodb::access_token_record::ScopeRecord::ProductsWrite
                    ]),
                    record.scopes
                );
                assert_eq!(60, record.expires - record.created.unix_timestamp());
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::put_item::PutItemOutput::builder().build())
                })
            });
        let mut user_service = MockUserService::default();
        user_service
            .expect_create_access_token()
            .return_once(|_, user_id, cmd| {
                let raw = RawAccessToken::new();
                let now = OffsetDateTime::now_utc();
                let token = AccessToken {
                    id: AccessTokenId::new(),
                    hashed_token: raw.clone().into(),
                    user_id: *user_id,
                    name: cmd.name,
                    scopes: cmd.scopes,
                    origin: cmd.origin,
                    expires: cmd.expires,
                    created_by: common::actor::domain::Actor::User(*user_id),
                    updated_by: common::actor::domain::Actor::User(*user_id),
                    created: now,
                    updated: now,
                };
                Box::pin(async move { Ok((raw, token)) })
            });
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let response = service
            .token(TokenRequest {
                grant_type: OAuthGrantType::AuthorizationCode,
                code: code.code,
                redirect_uri: code.redirect_uri,
                client_id: client.client_id,
                client_secret: secret,
                code_verifier: OAuthCodeVerifier::from(verifier),
            })
            .await
            .unwrap();

        assert_eq!(HashSet::from([Scope::ProductsWrite]), response.scopes);
        assert_eq!(None, response.expires);
        assert!(response.third_party_exchange_code.is_some());
    }

    #[tokio::test]
    async fn should_exchange_third_party_exchange_code_for_access_token() {
        let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
            .unwrap();
        let code = ThirdPartyExchangeCode::new();
        let raw = RawAccessToken::new();
        let grant = ThirdPartyExchangeCodeGrant {
            code,
            access_token: raw.clone(),
            access_token_expires: Some(now + time::Duration::hours(1)),
            scopes: HashSet::from([Scope::ProductsWrite]),
            expires: now + THIRD_PARTY_EXCHANGE_CODE_TTL,
            created: now,
        };
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_third_party_exchange_code_record()
            .return_once(move |actual_code| {
                assert_eq!(&code, actual_code);
                Box::pin(async move { Ok(Some(ThirdPartyExchangeCodeRecord::from(grant))) })
            });
        repository
            .expect_delete_third_party_exchange_code_record()
            .return_once(move |actual_code| {
                assert_eq!(&code, actual_code);
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder()
                            .build(),
                    )
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let response = service.token_by_third_party_code(&code).await.unwrap();

        assert_eq!(raw, response.access_token);
        assert_eq!(Some(now + time::Duration::hours(1)), response.expires);
        assert_eq!(HashSet::from([Scope::ProductsWrite]), response.scopes);
        assert!(response.third_party_exchange_code.is_none());
    }

    #[tokio::test]
    async fn should_reject_missing_third_party_exchange_code() {
        let code = ThirdPartyExchangeCode::new();
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_third_party_exchange_code_record()
            .return_once(move |actual_code| {
                assert_eq!(&code, actual_code);
                Box::pin(async { Ok(None) })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service.token_by_third_party_code(&code).await.unwrap_err();

        assert!(matches!(
            err,
            OAuthServiceError::ThirdPartyExchangeCodeNotFound
        ));
    }

    #[tokio::test]
    async fn should_reject_expired_third_party_exchange_code() {
        let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
            .unwrap();
        let code = ThirdPartyExchangeCode::new();
        let grant = ThirdPartyExchangeCodeGrant {
            code,
            access_token: RawAccessToken::new(),
            access_token_expires: None,
            scopes: HashSet::from([Scope::ProductsWrite]),
            expires: now - time::Duration::seconds(1),
            created: now - THIRD_PARTY_EXCHANGE_CODE_TTL,
        };
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_third_party_exchange_code_record()
            .return_once(move |actual_code| {
                assert_eq!(&code, actual_code);
                Box::pin(async move { Ok(Some(ThirdPartyExchangeCodeRecord::from(grant))) })
            });
        repository
            .expect_delete_third_party_exchange_code_record()
            .return_once(move |actual_code| {
                assert_eq!(&code, actual_code);
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder()
                            .build(),
                    )
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service.token_by_third_party_code(&code).await.unwrap_err();

        assert!(matches!(
            err,
            OAuthServiceError::ThirdPartyExchangeCodeExpired
        ));
    }

    #[tokio::test]
    async fn should_propagate_get_error_when_exchanging_third_party_exchange_code() {
        let code = ThirdPartyExchangeCode::new();
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_third_party_exchange_code_record()
            .return_once(|_| {
                Box::pin(async {
                    Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                        "get failed",
                    ))
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service.token_by_third_party_code(&code).await.unwrap_err();

        assert!(matches!(err, OAuthServiceError::SdkGetItemError(_)));
    }

    #[tokio::test]
    async fn should_propagate_delete_error_when_exchanging_third_party_exchange_code() {
        let now = OffsetDateTime::from_unix_timestamp(OffsetDateTime::now_utc().unix_timestamp())
            .unwrap();
        let code = ThirdPartyExchangeCode::new();
        let grant = ThirdPartyExchangeCodeGrant {
            code,
            access_token: RawAccessToken::new(),
            access_token_expires: None,
            scopes: HashSet::from([Scope::ProductsWrite]),
            expires: now + THIRD_PARTY_EXCHANGE_CODE_TTL,
            created: now,
        };
        let mut repository = MockOAuthRepository::default();
        repository
            .expect_get_third_party_exchange_code_record()
            .return_once(move |_| {
                Box::pin(async move { Ok(Some(ThirdPartyExchangeCodeRecord::from(grant))) })
            });
        repository
            .expect_delete_third_party_exchange_code_record()
            .return_once(|_| {
                Box::pin(async {
                    Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                        "delete failed",
                    ))
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service.token_by_third_party_code(&code).await.unwrap_err();

        assert!(matches!(err, OAuthServiceError::SdkDeleteItemError(_)));
    }

    #[tokio::test]
    async fn should_reject_expired_authorization_code() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let now = OffsetDateTime::now_utc();
        let code = AuthorizationCode {
            code: OAuthAuthorizationCode::new(),
            client_id: client.client_id,
            user_id: UserId::new(),
            redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
            scopes: HashSet::from([Scope::ProductsWrite]),
            code_challenge: OAuthCodeChallenge::from("challenge"),
            code_challenge_method: CodeChallengeMethod::S256,
            expires: now - time::Duration::minutes(1),
            created: now,
        };
        let client_record_secret = secret.clone();
        let mut repository = MockOAuthRepository::default();
        repository.expect_get_client_record().return_once(move |_| {
            Box::pin(async move {
                Ok(Some(OAuthClientRecord::from((
                    client,
                    client_record_secret,
                ))))
            })
        });
        repository
            .expect_get_authorization_code_record()
            .return_once(move |_| {
                Box::pin(async move { Ok(Some(AuthorizationCodeRecord::from(code))) })
            });
        repository
            .expect_delete_authorization_code_record()
            .return_once(|_| {
                Box::pin(async {
                    Ok(
                        aws_sdk_dynamodb::operation::delete_item::DeleteItemOutput::builder()
                            .build(),
                    )
                })
            });
        let user_service = MockUserService::default();
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let err = service
            .token(TokenRequest {
                grant_type: OAuthGrantType::AuthorizationCode,
                code: OAuthAuthorizationCode::new(),
                redirect_uri: url::Url::parse("https://client.example/callback").unwrap(),
                client_id: client_id(),
                client_secret: secret,
                code_verifier: OAuthCodeVerifier::from("verifier"),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, OAuthServiceError::AuthorizationCodeExpired));
    }

    #[tokio::test]
    async fn should_revoke_oauth_access_token() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let client_record_secret = secret.clone();
        let mut repository = MockOAuthRepository::default();
        repository.expect_get_client_record().return_once(move |_| {
            Box::pin(async move {
                Ok(Some(OAuthClientRecord::from((
                    client,
                    client_record_secret,
                ))))
            })
        });
        let mut user_service = MockUserService::default();
        user_service
            .expect_delete_access_token_by_raw()
            .return_once(|_, _| Box::pin(async { Ok(()) }));
        let service = OAuthServiceImpl::new(&repository, &user_service);

        service
            .revoke(
                &RequestContext {
                    actor: Actor::System,
                },
                TokenRevocationRequest {
                    token: RawAccessToken::new(),
                    client_id: client_id(),
                    client_secret: secret,
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_introspect_active_oauth_access_token() {
        let secret = RawOAuthClientSecret::new();
        let client = oauth_client(&secret);
        let raw = RawAccessToken::new();
        let now = OffsetDateTime::now_utc();
        let user_id = UserId::new();
        let access_token = AccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.clone().into(),
            user_id,
            name: "OAuth token".into(),
            scopes: HashSet::from([Scope::ProductsWrite]),
            origin: AccessTokenOrigin::OAuth {
                client_id: client.client_id,
            },
            expires: None,
            created_by: common::actor::domain::Actor::User(user_id),
            updated_by: common::actor::domain::Actor::User(user_id),
            created: now,
            updated: now,
        };
        let client_record_secret = secret.clone();
        let mut repository = MockOAuthRepository::default();
        repository.expect_get_client_record().return_once(move |_| {
            Box::pin(async move {
                Ok(Some(OAuthClientRecord::from((
                    client,
                    client_record_secret,
                ))))
            })
        });
        let mut user_service = MockUserService::default();
        user_service
            .expect_find_access_token_by_raw()
            .return_once(move |_| Box::pin(async move { Ok(access_token) }));
        let service = OAuthServiceImpl::new(&repository, &user_service);

        let response = service
            .introspect(TokenIntrospectionRequest {
                token: raw,
                client_id: client_id(),
                client_secret: secret,
            })
            .await
            .unwrap();

        assert!(response.active);
        assert_eq!(Some(client_id()), response.client_id);
    }
}
