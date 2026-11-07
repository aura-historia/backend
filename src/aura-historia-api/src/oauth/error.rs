use super::no_store;
use application::error::{BoxError, box_error};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oauth_service::error::OAuthServiceError;
use serde::{Serialize, Serializer};
use std::fmt;

/// Protocol errors never expose submitted credentials or technical failure causes.
#[derive(thiserror::Error)]
pub(crate) enum OAuthError {
    #[error("invalid_request")]
    InvalidRequest,
    #[error("invalid_client")]
    InvalidClient,
    #[error("invalid_grant")]
    InvalidGrant,
    #[error("unsupported_grant_type")]
    UnsupportedGrantType,
    /// The client may not use the requested grant type.
    #[error("unauthorized_client")]
    UnauthorizedClient,
    #[error("unsupported_response_type")]
    UnsupportedResponseType,
    #[error("invalid_scope")]
    InvalidScope,
    #[error("invalid_target")]
    InvalidTarget,
    #[error("access_denied")]
    AccessDenied,
    #[error("server_error")]
    ServerError {
        #[source]
        source: BoxError,
    },
    #[error("temporarily_unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
}

// The cause remains available through Error::source, but routine error formatting
// must not expose credentials or provider content carried by an adapter failure.
impl fmt::Debug for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Serialize for OAuthError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl From<OAuthServiceError> for OAuthError {
    fn from(error: OAuthServiceError) -> Self {
        match error {
            OAuthServiceError::ClientNotFound
            | OAuthServiceError::InvalidClientSecret
            | OAuthServiceError::InvalidMetadataDocumentClient { .. } => Self::InvalidClient,
            OAuthServiceError::AuthorizationCodeNotFound
            | OAuthServiceError::AuthorizationCodeExpired
            | OAuthServiceError::AuthorizationCodeClientMismatch
            | OAuthServiceError::AuthorizationCodeRedirectUriMismatch
            | OAuthServiceError::InvalidCodeVerifier
            | OAuthServiceError::ConnectionNotFound
            | OAuthServiceError::RefreshTokenInvalid
            | OAuthServiceError::RefreshTokenExpired
            | OAuthServiceError::RefreshTokenReplaced
            | OAuthServiceError::RefreshTokenReused
            | OAuthServiceError::ThirdPartyExchangeCodeNotFound
            | OAuthServiceError::ThirdPartyExchangeCodeExpired => Self::InvalidGrant,
            OAuthServiceError::InvalidResource
            | OAuthServiceError::AuthorizationCodeResourceMismatch
            | OAuthServiceError::RefreshTokenResourceMismatch => Self::InvalidTarget,
            OAuthServiceError::InvalidScope => Self::InvalidScope,
            OAuthServiceError::InvalidRedirectUri | OAuthServiceError::InvalidClientMetadata(_) => {
                Self::InvalidRequest
            }
            OAuthServiceError::AuthenticatedActorRequired
            | OAuthServiceError::Forbidden
            | OAuthServiceError::DelegatedAuthorizationForbidden
            | OAuthServiceError::DelegatedClientRegistrationForbidden
            | OAuthServiceError::DelegatedDisconnectForbidden => Self::AccessDenied,
            error @ OAuthServiceError::TemporarilyUnavailable { .. } => {
                Self::TemporarilyUnavailable {
                    source: box_error(error),
                }
            }
            error @ (OAuthServiceError::ConcurrencyConflict
            | OAuthServiceError::InvalidPersistedState { .. }
            | OAuthServiceError::Internal { .. }) => Self::ServerError {
                source: box_error(error),
            },
        }
    }
}

impl OAuthError {
    fn status(&self) -> StatusCode {
        match self {
            Self::InvalidClient => StatusCode::UNAUTHORIZED,
            Self::AccessDenied => StatusCode::FORBIDDEN,
            Self::ServerError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::TemporarilyUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_REQUEST,
        }
    }

    /// The protocol error with an endpoint-specific status.
    pub(crate) fn into_response_with_status(self, status: StatusCode) -> Response {
        no_store((status, Json(serde_json::json!({ "error": self }))).into_response())
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let status = self.status();
        self.into_response_with_status(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::test_support::{SECRET_MARKER, secret_free_problem};
    use std::error::Error;

    #[tokio::test]
    async fn should_map_client_and_code_failures_to_standard_oauth_errors() {
        for (error, status, code) in [
            (OAuthServiceError::ClientNotFound, 401, "invalid_client"),
            (
                OAuthServiceError::InvalidClientSecret,
                401,
                "invalid_client",
            ),
            (
                OAuthServiceError::AuthorizationCodeNotFound,
                400,
                "invalid_grant",
            ),
            (
                OAuthServiceError::AuthorizationCodeExpired,
                400,
                "invalid_grant",
            ),
            (
                OAuthServiceError::AuthorizationCodeClientMismatch,
                400,
                "invalid_grant",
            ),
            (
                OAuthServiceError::AuthorizationCodeRedirectUriMismatch,
                400,
                "invalid_grant",
            ),
            (OAuthServiceError::InvalidCodeVerifier, 400, "invalid_grant"),
            (OAuthServiceError::ConnectionNotFound, 400, "invalid_grant"),
            (OAuthServiceError::RefreshTokenInvalid, 400, "invalid_grant"),
            (OAuthServiceError::RefreshTokenExpired, 400, "invalid_grant"),
            (
                OAuthServiceError::RefreshTokenReplaced,
                400,
                "invalid_grant",
            ),
            (OAuthServiceError::RefreshTokenReused, 400, "invalid_grant"),
            (
                OAuthServiceError::RefreshTokenResourceMismatch,
                400,
                "invalid_target",
            ),
            (OAuthServiceError::InvalidScope, 400, "invalid_scope"),
            (
                OAuthServiceError::InvalidMetadataDocumentClient {
                    source: box_error(std::io::Error::other(SECRET_MARKER)),
                },
                401,
                "invalid_client",
            ),
        ] {
            let (actual_status, body) =
                secret_free_problem(OAuthError::from(error), SECRET_MARKER).await;
            assert_eq!(status, actual_status);
            assert_eq!(serde_json::json!({"error": code}), body);
        }
    }

    #[tokio::test]
    async fn should_keep_technical_failure_causes_out_of_protocol_responses() {
        for (error, status, code) in [
            (
                OAuthServiceError::Internal {
                    source: application::error::box_error(std::io::Error::other(SECRET_MARKER)),
                },
                500,
                "server_error",
            ),
            (
                OAuthServiceError::InvalidPersistedState {
                    source: box_error(std::io::Error::other(SECRET_MARKER)),
                },
                500,
                "server_error",
            ),
            (
                OAuthServiceError::TemporarilyUnavailable {
                    source: application::error::box_error(std::io::Error::other(SECRET_MARKER)),
                },
                503,
                "temporarily_unavailable",
            ),
        ] {
            let error = OAuthError::from(error);
            let service_error = error
                .source()
                .unwrap()
                .downcast_ref::<OAuthServiceError>()
                .unwrap();
            assert!(service_error.source().unwrap().is::<std::io::Error>());
            assert_eq!(code, error.to_string());
            assert_eq!(code, format!("{error:?}"));
            let (actual_status, body) = secret_free_problem(error, SECRET_MARKER).await;
            assert_eq!(status, actual_status);
            assert_eq!(serde_json::json!({"error": code}), body);
        }
    }

    #[tokio::test]
    async fn should_preserve_transaction_failure_chain_without_exposing_it() {
        let transaction = application::transaction::TransactionError::CommitFailed(box_error(
            std::io::Error::other(SECRET_MARKER),
        ));
        let error = OAuthError::from(OAuthServiceError::from(transaction));
        let service = error.source().unwrap();
        let transaction = service.source().unwrap();
        assert!(transaction.is::<application::transaction::TransactionError>());
        assert!(transaction.source().unwrap().is::<std::io::Error>());
        assert_eq!("temporarily_unavailable", error.to_string());
        assert_eq!("temporarily_unavailable", format!("{error:?}"));
        let (status, body) = secret_free_problem(error, SECRET_MARKER).await;
        assert_eq!(503, status);
        assert_eq!(
            serde_json::json!({"error": "temporarily_unavailable"}),
            body
        );
    }
}
