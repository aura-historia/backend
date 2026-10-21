use super::no_store;
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use oauth_core::resource::AuraResourcePolicy;
use serde::Serialize;
use user_core::access_token::Scope;

#[derive(Serialize)]
struct AuthorizationServerMetadata {
    issuer: &'static str,
    authorization_endpoint: String,
    token_endpoint: String,
    response_types_supported: [&'static str; 1],
    response_modes_supported: [&'static str; 1],
    grant_types_supported: [&'static str; 1],
    token_endpoint_auth_methods_supported: [&'static str; 1],
    code_challenge_methods_supported: [&'static str; 1],
    scopes_supported: [&'static str; 8],
}

impl From<AuraResourcePolicy> for AuthorizationServerMetadata {
    fn from(policy: AuraResourcePolicy) -> Self {
        let issuer = policy.issuer();
        // Both closed policy environments share the corresponding browser host.
        let browser_origin = issuer.replacen("https://api.", "https://", 1);
        Self {
            issuer,
            authorization_endpoint: format!("{browser_origin}/en/oauth/authorize"),
            token_endpoint: format!("{issuer}/v1/oauth/token"),
            response_types_supported: ["code"],
            response_modes_supported: ["query"],
            grant_types_supported: ["authorization_code"],
            token_endpoint_auth_methods_supported: ["client_secret_post"],
            code_challenge_methods_supported: ["S256"],
            scopes_supported: [
                Scope::UsersRead,
                Scope::UsersWrite,
                Scope::SearchFiltersRead,
                Scope::SearchFiltersWrite,
                Scope::WatchlistRead,
                Scope::WatchlistWrite,
                Scope::NotificationsRead,
                Scope::NotificationsWrite,
            ]
            .map(Scope::as_str),
        }
    }
}

pub(crate) async fn discovery(State(state): State<OAuthState>) -> Response {
    metadata_response(state.resource_policy)
}

fn metadata_response(policy: AuraResourcePolicy) -> Response {
    no_store(Json(AuthorizationServerMetadata::from(policy)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use serde_json::json;

    #[tokio::test]
    async fn should_advertise_only_implemented_features_for_each_environment() {
        for (policy, issuer, browser) in [
            (
                AuraResourcePolicy::production(),
                "https://api.aura-historia.com",
                "https://aura-historia.com",
            ),
            (
                AuraResourcePolicy::stage(),
                "https://api.stage.aura-historia.com",
                "https://stage.aura-historia.com",
            ),
        ] {
            let response = metadata_response(policy);
            assert_eq!(200, response.status());
            assert_eq!("application/json", response.headers()[header::CONTENT_TYPE]);
            assert_eq!("no-store", response.headers()[header::CACHE_CONTROL]);
            assert_eq!("no-cache", response.headers()[header::PRAGMA]);
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                json!({
                    "issuer": issuer,
                    "authorization_endpoint": format!("{browser}/en/oauth/authorize"),
                    "token_endpoint": format!("{issuer}/v1/oauth/token"),
                    "response_types_supported": ["code"],
                    "response_modes_supported": ["query"],
                    "grant_types_supported": ["authorization_code"],
                    "token_endpoint_auth_methods_supported": ["client_secret_post"],
                    "code_challenge_methods_supported": ["S256"],
                    "scopes_supported": ["users:read", "users:write", "search-filters:read", "search-filters:write", "watchlist:read", "watchlist:write", "notifications:read", "notifications:write"],
                }),
                actual
            );
        }
    }
}
