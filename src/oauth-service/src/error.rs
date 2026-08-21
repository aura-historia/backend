use crate::ports::{
    OAuthAccessTokenGatewayError, OAuthClientRepositoryError, OAuthCodeRepositoryError,
};
use application::operation_context::{AuthenticationRequired, OperationAuthorizationError};

#[derive(Debug, thiserror::Error)]
pub enum OAuthServiceError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("OAuth client not found")]
    ClientNotFound,
    #[error("Invalid OAuth client secret")]
    InvalidClientSecret,
    #[error("Redirect URI is not registered for client")]
    InvalidRedirectUri,
    #[error("Requested scope is not allowed for client")]
    InvalidScope,
    #[error("Authorization code not found")]
    AuthorizationCodeNotFound,
    #[error("Authorization code expired")]
    AuthorizationCodeExpired,
    #[error("Authorization code does not belong to client")]
    AuthorizationCodeClientMismatch,
    #[error("Authorization code redirect_uri mismatch")]
    AuthorizationCodeRedirectUriMismatch,
    #[error("PKCE code_verifier did not match code_challenge")]
    InvalidCodeVerifier,
    #[error("Third-party exchange code not found")]
    ThirdPartyExchangeCodeNotFound,
    #[error("Third-party exchange code expired")]
    ThirdPartyExchangeCodeExpired,
    #[error("OAuth client metadata is invalid: {0}")]
    InvalidClientMetadata(String),
    #[error("temporary OAuth failure")]
    TemporarilyUnavailable,
    #[error("invalid persisted OAuth state")]
    InvalidPersistedState,
    #[error("internal OAuth failure")]
    Internal,
}

impl From<AuthenticationRequired> for OAuthServiceError {
    fn from(_: AuthenticationRequired) -> Self {
        Self::AuthenticatedActorRequired
    }
}

impl From<OperationAuthorizationError> for OAuthServiceError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}

impl From<OAuthClientRepositoryError> for OAuthServiceError {
    fn from(error: OAuthClientRepositoryError) -> Self {
        match error {
            OAuthClientRepositoryError::Conflict { .. }
            | OAuthClientRepositoryError::Internal { .. } => Self::Internal,
            OAuthClientRepositoryError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable
            }
            OAuthClientRepositoryError::InvalidPersistedState { .. } => Self::InvalidPersistedState,
        }
    }
}

impl From<OAuthCodeRepositoryError> for OAuthServiceError {
    fn from(error: OAuthCodeRepositoryError) -> Self {
        match error {
            OAuthCodeRepositoryError::Conflict { .. }
            | OAuthCodeRepositoryError::Internal { .. } => Self::Internal,
            OAuthCodeRepositoryError::TemporarilyUnavailable { .. } => Self::TemporarilyUnavailable,
            OAuthCodeRepositoryError::InvalidPersistedState { .. } => Self::InvalidPersistedState,
        }
    }
}

impl From<OAuthAccessTokenGatewayError> for OAuthServiceError {
    fn from(error: OAuthAccessTokenGatewayError) -> Self {
        match error {
            OAuthAccessTokenGatewayError::NotFound => Self::AuthorizationCodeNotFound,
            OAuthAccessTokenGatewayError::Expired => Self::AuthorizationCodeExpired,
            OAuthAccessTokenGatewayError::Forbidden => Self::Forbidden,
            OAuthAccessTokenGatewayError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable
            }
            OAuthAccessTokenGatewayError::InvalidPersistedState { .. } => {
                Self::InvalidPersistedState
            }
            OAuthAccessTokenGatewayError::Internal { .. } => Self::Internal,
        }
    }
}
