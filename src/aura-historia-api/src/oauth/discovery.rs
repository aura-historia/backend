use super::no_store;
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use oauth_core::metadata_document_client::MetadataDocumentClientPolicy;
use oauth_core::ordinary_user_scopes::ORDINARY_USER_SCOPES;
use oauth_core::refresh_token::OFFLINE_ACCESS_SCOPE;
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
    grant_types_supported: &'static [&'static str],
    token_endpoint_auth_methods_supported: &'static [&'static str],
    code_challenge_methods_supported: [&'static str; 1],
    scopes_supported: Vec<&'static str>,
    authorization_response_iss_parameter_supported: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    client_id_metadata_document_supported: bool,
}

impl AuthorizationServerMetadata {
    /// Metadata document clients, and the refresh tokens only they receive, are advertised
    /// only while the operator enables them. `offline_access` asks for a refresh token; the
    /// MCP protected-resource metadata never lists it.
    fn new(
        resource_policy: AuraResourcePolicy,
        metadata_document_clients: &MetadataDocumentClientPolicy,
    ) -> Self {
        let issuer = resource_policy.issuer();
        let metadata_document_clients = metadata_document_clients.is_enabled();
        Self {
            issuer,
            authorization_endpoint: format!("{}/en/oauth/authorize", resource_policy.site_origin()),
            token_endpoint: format!("{issuer}/v1/oauth/token"),
            response_types_supported: ["code"],
            response_modes_supported: ["query"],
            grant_types_supported: if metadata_document_clients {
                &["authorization_code", "refresh_token"]
            } else {
                &["authorization_code"]
            },
            token_endpoint_auth_methods_supported: if metadata_document_clients {
                &["client_secret_post", "none"]
            } else {
                &["client_secret_post"]
            },
            code_challenge_methods_supported: ["S256"],
            scopes_supported: ORDINARY_USER_SCOPES
                .map(Scope::as_str)
                .into_iter()
                .chain(metadata_document_clients.then_some(OFFLINE_ACCESS_SCOPE))
                .collect(),
            authorization_response_iss_parameter_supported: true,
            client_id_metadata_document_supported: metadata_document_clients,
        }
    }
}

pub(crate) async fn discovery(State(state): State<OAuthState>) -> Response {
    metadata_response(
        state.resource_policy,
        &state.metadata_document_client_policy,
    )
}

fn metadata_response(
    resource_policy: AuraResourcePolicy,
    metadata_document_clients: &MetadataDocumentClientPolicy,
) -> Response {
    no_store(
        Json(AuthorizationServerMetadata::new(
            resource_policy,
            metadata_document_clients,
        ))
        .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use serde_json::json;

    async fn metadata(
        policy: AuraResourcePolicy,
        metadata_document_clients: MetadataDocumentClientPolicy,
    ) -> serde_json::Value {
        let response = metadata_response(policy, &metadata_document_clients);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn should_advertise_metadata_document_clients_only_when_enabled() {
        let enabled = metadata(
            AuraResourcePolicy::stage(),
            MetadataDocumentClientPolicy::Enabled {
                denied_hosts: Default::default(),
            },
        )
        .await;
        assert_eq!(
            json!(true),
            enabled["client_id_metadata_document_supported"]
        );
        assert_eq!(
            json!(["client_secret_post", "none"]),
            enabled["token_endpoint_auth_methods_supported"]
        );
        assert_eq!(
            json!(["authorization_code", "refresh_token"]),
            enabled["grant_types_supported"]
        );
        assert_eq!(
            json!("offline_access"),
            enabled["scopes_supported"][8],
            "{enabled}"
        );
        assert_eq!(
            json!(true),
            enabled["authorization_response_iss_parameter_supported"]
        );

        let disabled = metadata(
            AuraResourcePolicy::stage(),
            MetadataDocumentClientPolicy::Disabled,
        )
        .await;
        assert!(
            disabled
                .get("client_id_metadata_document_supported")
                .is_none()
        );
        assert_eq!(
            json!(["client_secret_post"]),
            disabled["token_endpoint_auth_methods_supported"]
        );
        assert_eq!(
            json!(["authorization_code"]),
            disabled["grant_types_supported"]
        );
        assert_eq!(8, disabled["scopes_supported"].as_array().unwrap().len());
        assert_eq!(
            json!(true),
            disabled["authorization_response_iss_parameter_supported"]
        );
    }

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
            let response = metadata_response(policy, &MetadataDocumentClientPolicy::Disabled);
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
                    "authorization_response_iss_parameter_supported": true,
                }),
                actual
            );
        }
    }
}
