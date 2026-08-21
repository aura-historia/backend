use crate::{
    core::access_token::{
        AccessToken, AccessTokenPrefix, RawAccessToken, TokenPrefix, api::ExtractBearerTokenError,
    },
    service::user_service::{UserService, UserServiceError},
};
use cognito_verifier::access_token_verifier_service::{
    AccessTokenVerifierError, AccessTokenVerifierService,
};
use common::user_id::UserId;
use http::HeaderMap;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum AuthenticatedPrincipal {
    UserId(UserId),
    AccessToken(AccessToken),
}

impl AuthenticatedPrincipal {
    pub fn user_id(&self) -> UserId {
        match self {
            AuthenticatedPrincipal::UserId(user_id) => *user_id,
            AuthenticatedPrincipal::AccessToken(access_token) => access_token.user_id,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, thiserror::Error)]
pub enum AuthenticatorError {
    #[error("Failed to extract bearer token from headers: {0}")]
    ExtractBearerTokenError(#[from] ExtractBearerTokenError),

    #[error("Cognito access-token verification failed: {0}")]
    AccessTokenVerifier(#[from] AccessTokenVerifierError),

    #[error("User access-token lookup failed: {0}")]
    UserService(#[from] UserServiceError),

    #[error("Invalid Aura Historia access-token: {0}")]
    InvalidRawAccessToken(#[from] crate::core::access_token::InvalidRawAccessTokenError),
}

#[cfg(feature = "data")]
pub mod api {
    use super::{AccessTokenVerifierError, AuthenticatorError};
    use common::api::{
        error::ApiError,
        error_code::{BAD_HEADER_VALUE, INTERNAL_SERVER_ERROR, INVALID_UUID, UNAUTHORIZED},
    };
    use http::header::AUTHORIZATION;

    impl From<AuthenticatorError> for ApiError {
        fn from(value: AuthenticatorError) -> Self {
            match value {
                AuthenticatorError::ExtractBearerTokenError(err) => err.into(),
                AuthenticatorError::AccessTokenVerifier(err) => map_access_token_error(err),
                AuthenticatorError::UserService(err) => err.into(),
                AuthenticatorError::InvalidRawAccessToken(err) => {
                    ApiError::unauthorized(UNAUTHORIZED)
                        .with_header_field("Authorization")
                        .with_detail(err.to_string())
                }
            }
        }
    }

    fn map_access_token_error(value: AccessTokenVerifierError) -> ApiError {
        match value {
            AccessTokenVerifierError::HttpHeaderValueToStrError(ref error) => {
                let detail = error.to_string();
                ApiError::bad_request(BAD_HEADER_VALUE, Box::new(value))
                    .with_header_field(AUTHORIZATION.as_str())
                    .with_detail(detail)
            }
            AccessTokenVerifierError::JwtCognito(_)
            | AccessTokenVerifierError::JwtError(_)
            | AccessTokenVerifierError::JwksFetchError(_)
            | AccessTokenVerifierError::ClaimIsNotString(_) => {
                ApiError::internal_server_error(INTERNAL_SERVER_ERROR, Box::new(value))
            }
            AccessTokenVerifierError::MissingClaim(claim) => {
                ApiError::bad_request(BAD_HEADER_VALUE, Box::new(value))
                    .with_header_field(AUTHORIZATION.as_str())
                    .with_detail(format!("Missing claim '{claim}'."))
            }
            AccessTokenVerifierError::InvalidUuid(claim, _) => {
                ApiError::bad_request(INVALID_UUID, Box::new(value)).with_detail(format!(
                    "String-Value for decoded claim '{claim}' is not a valid UUID."
                ))
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait AuthenticatorService {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<AuthenticatedPrincipal>, AuthenticatorError>;
}

pub struct AuthenticatorServiceImpl<'a> {
    access_token_verifier_service: &'a (dyn AccessTokenVerifierService + Sync),
    user_service: &'a (dyn UserService + Sync),
}

impl<'a> AuthenticatorServiceImpl<'a> {
    pub fn new(
        access_token_verifier_service: &'a (dyn AccessTokenVerifierService + Sync),
        user_service: &'a (dyn UserService + Sync),
    ) -> Self {
        Self {
            access_token_verifier_service,
            user_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> AuthenticatorService for AuthenticatorServiceImpl<'a> {
    async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<AuthenticatedPrincipal>, AuthenticatorError> {
        let Some(access_token) = crate::core::access_token::api::extract_bearer_token(headers)?
        else {
            return Ok(None);
        };

        if access_token.starts_with(AccessTokenPrefix::PREFIX) {
            let raw_access_token = RawAccessToken::try_from(access_token)?;
            let access_token = self
                .user_service
                .find_access_token_by_raw(&raw_access_token)
                .await?;
            Ok(Some(AuthenticatedPrincipal::AccessToken(access_token)))
        } else {
            let user_id = self
                .access_token_verifier_service
                .verify_extract_user_id_from_access_token(&access_token)
                .await?;
            Ok(Some(AuthenticatedPrincipal::UserId(user_id)))
        }
    }
}
