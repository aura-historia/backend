use super::AgentDiscoveryState;
use crate::{error::BearerAuthenticationFailure, oauth::no_store};
use axum::{
    Json,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use oauth_core::{ordinary_user_scopes::ORDINARY_USER_SCOPES, resource::AuraResourcePolicy};
use serde::Serialize;
use user_core::access_token::Scope;

pub(crate) const PROTECTED_RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource";

/// [RFC 9728](https://www.rfc-editor.org/rfc/rfc9728) metadata for the shared Aura default
/// resource that REST serves. It advertises the ordinary-user vocabulary, like
/// authorization-server discovery; a client's own scope ceiling remains authoritative.
#[derive(Serialize)]
struct ProtectedResourceMetadata {
    resource: &'static str,
    authorization_servers: [&'static str; 1],
    bearer_methods_supported: [&'static str; 1],
    scopes_supported: [&'static str; 8],
}

impl ProtectedResourceMetadata {
    fn new(policy: AuraResourcePolicy) -> Self {
        Self {
            resource: policy.rest_resource(),
            authorization_servers: [policy.issuer()],
            bearer_methods_supported: ["header"],
            scopes_supported: ORDINARY_USER_SCOPES.map(Scope::as_str),
        }
    }
}

/// OAuth metadata is never cached, like authorization-server discovery.
pub(crate) async fn get_protected_resource_metadata(
    State(state): State<AgentDiscoveryState>,
) -> Response {
    no_store(Json(ProtectedResourceMetadata::new(state.resource_policy)).into_response())
}

pub(crate) fn metadata_url(policy: AuraResourcePolicy) -> String {
    format!(
        "{}{PROTECTED_RESOURCE_METADATA_PATH}",
        policy.rest_resource()
    )
}

/// The Bearer challenges that point a client at the metadata (RFC 9728 §5.1). A presented
/// credential that failed adds `error="invalid_token"` (RFC 6750 §3.1), so a client knows to
/// refresh or sign in again; a missing one names no error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BearerChallenge {
    missing: HeaderValue,
    invalid_token: HeaderValue,
}

impl BearerChallenge {
    pub(crate) fn new(policy: AuraResourcePolicy) -> Self {
        let missing = format!("Bearer resource_metadata=\"{}\"", metadata_url(policy));
        let invalid_token = format!("{missing}, error=\"invalid_token\"");
        let header = |value: String| {
            HeaderValue::try_from(value).expect("closed policy origins are valid header values")
        };
        Self {
            missing: header(missing),
            invalid_token: header(invalid_token),
        }
    }

    fn for_failure(&self, failure: BearerAuthenticationFailure) -> &HeaderValue {
        match failure {
            BearerAuthenticationFailure::Missing => &self.missing,
            BearerAuthenticationFailure::InvalidToken => &self.invalid_token,
        }
    }
}

/// Adds the challenge to a `401` whose bearer credential was missing or invalid. The router
/// applies this only to routes that accept Aura access tokens.
pub(crate) async fn add_bearer_challenge(
    challenge: BearerChallenge,
    mut response: Response,
) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED
        && let Some(failure) = response
            .extensions()
            .get::<BearerAuthenticationFailure>()
            .copied()
    {
        let value = challenge.for_failure(failure).clone();
        response
            .headers_mut()
            .insert(header::WWW_AUTHENTICATE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthError;
    use crate::error::{ApiError, FORBIDDEN, INVALID_CREDENTIALS, LOOPS_WEBHOOK_UNAUTHORIZED};
    use serde_json::{Value, json};

    async fn body(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn should_describe_the_shared_default_resource_of_each_environment_without_caching() {
        for (policy, origin) in [
            (
                AuraResourcePolicy::production(),
                "https://api.aura-historia.com",
            ),
            (
                AuraResourcePolicy::stage(),
                "https://api.stage.aura-historia.com",
            ),
        ] {
            let response =
                get_protected_resource_metadata(State(AgentDiscoveryState::new(policy))).await;
            assert_eq!(StatusCode::OK, response.status());
            assert_eq!("application/json", response.headers()[header::CONTENT_TYPE]);
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            assert_eq!("no-cache", response.headers()[header::PRAGMA]);
            assert_eq!(
                json!({
                    "resource": origin,
                    "authorization_servers": [origin],
                    "bearer_methods_supported": ["header"],
                    "scopes_supported": ["users:read", "users:write", "search-filters:read", "search-filters:write", "watchlist:read", "watchlist:write", "notifications:read", "notifications:write"],
                }),
                body(response).await
            );
            let challenge = BearerChallenge::new(policy);
            let metadata = format!(
                "Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
            );
            assert_eq!(
                metadata,
                *challenge.for_failure(BearerAuthenticationFailure::Missing)
            );
            assert_eq!(
                format!("{metadata}, error=\"invalid_token\""),
                *challenge.for_failure(BearerAuthenticationFailure::InvalidToken)
            );
        }
    }

    #[tokio::test]
    async fn should_challenge_only_bearer_authentication_failures() {
        let challenge = BearerChallenge::new(AuraResourcePolicy::stage());
        for (error, failure) in [
            (
                ApiError::unauthorized(INVALID_CREDENTIALS),
                BearerAuthenticationFailure::Missing,
            ),
            (
                ApiError::from(AuthError::MissingCredentials),
                BearerAuthenticationFailure::Missing,
            ),
            (
                ApiError::from(AuthError::InvalidCredentials),
                BearerAuthenticationFailure::InvalidToken,
            ),
            (
                ApiError::from(AuthError::MalformedCredentials),
                BearerAuthenticationFailure::InvalidToken,
            ),
        ] {
            let challenged = add_bearer_challenge(challenge.clone(), error.into_response()).await;
            assert_eq!(
                challenge.for_failure(failure),
                challenged.headers()[header::WWW_AUTHENTICATE]
            );
        }

        for response in [
            ApiError::unauthorized(LOOPS_WEBHOOK_UNAUTHORIZED).into_response(),
            ApiError::forbidden(FORBIDDEN).into_response(),
            StatusCode::UNAUTHORIZED.into_response(),
        ] {
            let response = add_bearer_challenge(challenge.clone(), response).await;
            assert!(!response.headers().contains_key(header::WWW_AUTHENTICATE));
        }
    }
}
