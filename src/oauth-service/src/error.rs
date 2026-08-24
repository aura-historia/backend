use crate::ports::{OAuthClientReadError, OAuthClientRepositoryError, OAuthCodeRepositoryError};
use application::operation_context::{AuthenticationRequired, OperationAuthorizationError};
use application::transaction::TransactionError;
use user_service::ports::{AccessTokenAuthenticationReadError, AccessTokenRepositoryError};

#[derive(Debug, thiserror::Error)]
pub enum OAuthServiceError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("OAuth client not found")]
    ClientNotFound,
    #[error("concurrent OAuth client update")]
    ConcurrencyConflict,
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

impl From<OAuthClientReadError> for OAuthServiceError {
    fn from(error: OAuthClientReadError) -> Self {
        match error {
            OAuthClientReadError::TemporarilyUnavailable { .. } => Self::TemporarilyUnavailable,
            OAuthClientReadError::InvalidPersistedState { .. } => Self::InvalidPersistedState,
            OAuthClientReadError::Internal { .. } => Self::Internal,
        }
    }
}

impl From<OAuthClientRepositoryError> for OAuthServiceError {
    fn from(error: OAuthClientRepositoryError) -> Self {
        match error {
            OAuthClientRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
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

impl From<AccessTokenAuthenticationReadError> for OAuthServiceError {
    fn from(error: AccessTokenAuthenticationReadError) -> Self {
        match error {
            AccessTokenAuthenticationReadError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable
            }
            AccessTokenAuthenticationReadError::InvalidReadModel { .. } => {
                Self::InvalidPersistedState
            }
            AccessTokenAuthenticationReadError::Internal { .. } => Self::Internal,
        }
    }
}

impl From<AccessTokenRepositoryError> for OAuthServiceError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict
            | AccessTokenRepositoryError::Conflict { .. }
            | AccessTokenRepositoryError::Internal { .. } => Self::Internal,
            AccessTokenRepositoryError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable
            }
            AccessTokenRepositoryError::InvalidPersistedState { .. } => Self::InvalidPersistedState,
        }
    }
}

impl From<TransactionError> for OAuthServiceError {
    fn from(_: TransactionError) -> Self {
        Self::TemporarilyUnavailable
    }
}
