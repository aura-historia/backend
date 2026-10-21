use super::no_store;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use oauth_service::error::OAuthServiceError;
use serde::Serialize;

/// Protocol errors never expose submitted credentials or technical failure causes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OAuthError {
    InvalidRequest,
    InvalidClient,
    InvalidGrant,
    UnsupportedGrantType,
    UnsupportedResponseType,
    InvalidScope,
    InvalidTarget,
    AccessDenied,
    ServerError,
    TemporarilyUnavailable,
}

impl From<OAuthServiceError> for OAuthError {
    fn from(error: OAuthServiceError) -> Self {
        match error {
            OAuthServiceError::ClientNotFound | OAuthServiceError::InvalidClientSecret => {
                Self::InvalidClient
            }
            OAuthServiceError::AuthorizationCodeNotFound
            | OAuthServiceError::AuthorizationCodeExpired
            | OAuthServiceError::AuthorizationCodeClientMismatch
            | OAuthServiceError::AuthorizationCodeRedirectUriMismatch
            | OAuthServiceError::InvalidCodeVerifier
            | OAuthServiceError::ThirdPartyExchangeCodeNotFound
            | OAuthServiceError::ThirdPartyExchangeCodeExpired => Self::InvalidGrant,
            OAuthServiceError::InvalidResource
            | OAuthServiceError::AuthorizationCodeResourceMismatch => Self::InvalidTarget,
            OAuthServiceError::InvalidScope => Self::InvalidScope,
            OAuthServiceError::InvalidRedirectUri | OAuthServiceError::InvalidClientMetadata(_) => {
                Self::InvalidRequest
            }
            OAuthServiceError::AuthenticatedActorRequired
            | OAuthServiceError::Forbidden
            | OAuthServiceError::DelegatedAuthorizationForbidden
            | OAuthServiceError::DelegatedClientRegistrationForbidden => Self::AccessDenied,
            OAuthServiceError::TemporarilyUnavailable { .. } => Self::TemporarilyUnavailable,
            OAuthServiceError::ConcurrencyConflict
            | OAuthServiceError::InvalidPersistedState { .. }
            | OAuthServiceError::Internal { .. } => Self::ServerError,
        }
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::InvalidClient => StatusCode::UNAUTHORIZED,
            Self::AccessDenied => StatusCode::FORBIDDEN,
            Self::ServerError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::TemporarilyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::BAD_REQUEST,
        };
        no_store((status, Json(serde_json::json!({ "error": self }))).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::test_support::{SECRET_MARKER, secret_free_problem};

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
            (OAuthServiceError::InvalidScope, 400, "invalid_scope"),
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
                OAuthServiceError::TemporarilyUnavailable {
                    source: application::error::box_error(std::io::Error::other(SECRET_MARKER)),
                },
                503,
                "temporarily_unavailable",
            ),
        ] {
            let (actual_status, body) =
                secret_free_problem(OAuthError::from(error), SECRET_MARKER).await;
            assert_eq!(status, actual_status);
            assert_eq!(serde_json::json!({"error": code}), body);
        }
    }
}
