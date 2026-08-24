use crate::ports::{OAuthClientReadError, OAuthClientRepositoryError, OAuthCodeRepositoryError};
use application::error::{BoxError, box_error};
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
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted OAuth state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal OAuth failure")]
    Internal {
        #[source]
        source: BoxError,
    },
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
            OAuthClientReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            OAuthClientReadError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            OAuthClientReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<OAuthClientRepositoryError> for OAuthServiceError {
    fn from(error: OAuthClientRepositoryError) -> Self {
        match error {
            OAuthClientRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            OAuthClientRepositoryError::Conflict { source }
            | OAuthClientRepositoryError::Internal { source } => Self::Internal { source },
            OAuthClientRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            OAuthClientRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<OAuthCodeRepositoryError> for OAuthServiceError {
    fn from(error: OAuthCodeRepositoryError) -> Self {
        match error {
            OAuthCodeRepositoryError::Conflict { source }
            | OAuthCodeRepositoryError::Internal { source } => Self::Internal { source },
            OAuthCodeRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            OAuthCodeRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<AccessTokenAuthenticationReadError> for OAuthServiceError {
    fn from(error: AccessTokenAuthenticationReadError) -> Self {
        match error {
            AccessTokenAuthenticationReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenAuthenticationReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenAuthenticationReadError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<AccessTokenRepositoryError> for OAuthServiceError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            error @ AccessTokenRepositoryError::ConcurrencyConflict => Self::Internal {
                source: box_error(error),
            },
            AccessTokenRepositoryError::Conflict { source }
            | AccessTokenRepositoryError::Internal { source } => Self::Internal { source },
            AccessTokenRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<TransactionError> for OAuthServiceError {
    fn from(error: TransactionError) -> Self {
        Self::TemporarilyUnavailable {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use std::error::Error;

    fn assert_source(error: OAuthServiceError) {
        let source = error.source();
        assert!(source.is_some(), "service error must retain its source");
    }

    #[test]
    fn should_preserve_adapter_sources_through_oauth_service_errors() {
        assert_source(OAuthServiceError::from(OAuthClientReadError::Internal {
            source: box_error(std::io::Error::other("client read")),
        }));
        assert_source(OAuthServiceError::from(
            OAuthClientRepositoryError::Internal {
                source: box_error(std::io::Error::other("client repository")),
            },
        ));
        assert_source(OAuthServiceError::from(
            OAuthCodeRepositoryError::Internal {
                source: box_error(std::io::Error::other("code repository")),
            },
        ));
        assert_source(OAuthServiceError::from(
            AccessTokenAuthenticationReadError::Internal {
                source: box_error(std::io::Error::other("authentication read")),
            },
        ));
        assert_source(OAuthServiceError::from(
            AccessTokenRepositoryError::Internal {
                source: box_error(std::io::Error::other("access token repository")),
            },
        ));
    }
}
