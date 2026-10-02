use super::{no_store, scope_strings};
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_CREDENTIALS};
use crate::state::OAuthState;
use crate::wire::parse_path_object_id;
use application::operation_context::Principal;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use serde::Serialize;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OAuthClientConsentMetadataData {
    client_id: OAuthClientId,
    client_name: String,
    tos_uri: Url,
    policy_uri: Url,
    client_uri: Url,
    logo_uri: Url,
    redirect_uris: Vec<Url>,
    scope: Vec<String>,
}

impl From<oauth_service::ports::OAuthClientView> for OAuthClientConsentMetadataData {
    fn from(client: oauth_service::ports::OAuthClientView) -> Self {
        let mut redirect_uris = client.redirect_uris.into_iter().collect::<Vec<_>>();
        redirect_uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));

        Self {
            client_id: client.client_id,
            client_name: client.name.into(),
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            redirect_uris,
            scope: scope_strings(client.scopes),
        }
    }
}

pub async fn get_consent_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    if !matches!(&context.principal, Principal::User(_)) {
        return no_store(ApiError::unauthorized(INVALID_CREDENTIALS).into_response());
    }
    let client_id: OAuthClientId = match parse_path_object_id(&raw, "clientId", "OAuthClient") {
        Ok(value) => value,
        Err(error) => return no_store(error.into_response()),
    };

    match state.get_consent_client.execute(&context, &client_id).await {
        Ok(result) => no_store(Json(OAuthClientConsentMetadataData::from(result)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use credential_core::oauth_client_id::OAuthClientId;
    use oauth_core::client::OAuthClientName;
    use std::collections::HashSet;
    use time::OffsetDateTime;
    use user_core::access_token::Scope;

    fn view(
        redirect_uris: &[&str],
        scopes: HashSet<Scope>,
    ) -> oauth_service::ports::OAuthClientView {
        oauth_service::ports::OAuthClientView {
            client_id: OAuthClientId::new(),
            name: OAuthClientName::from("Consent application"),
            redirect_uris: redirect_uris
                .iter()
                .map(|uri| Url::parse(uri).unwrap_or_else(|error| panic!("invalid URL: {error}")))
                .collect(),
            tos_uri: Url::parse("https://integration.example/terms")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            policy_uri: Url::parse("https://integration.example/privacy")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            client_uri: Url::parse("https://integration.example")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            logo_uri: Url::parse("https://integration.example/logo.png")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            scopes,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn should_serialize_only_consent_metadata_with_exact_registered_uris_and_scopes() {
        let dto = OAuthClientConsentMetadataData::from(view(
            &[
                "https://integration.example/callback?mode=stage&next=%2Fcatalog",
                "https://integration.example/woocommerce/callback?shop=merchant.example",
            ],
            HashSet::from([Scope::ProductListingsWrite, Scope::UsersRead]),
        ));
        let value = serde_json::to_value(dto)
            .unwrap_or_else(|error| panic!("failed to serialize consent metadata: {error}"));

        assert_eq!(
            serde_json::json!({
                "client_id": value["client_id"],
                "client_name": "Consent application",
                "tos_uri": "https://integration.example/terms",
                "policy_uri": "https://integration.example/privacy",
                "client_uri": "https://integration.example/",
                "logo_uri": "https://integration.example/logo.png",
                "redirect_uris": [
                    "https://integration.example/callback?mode=stage&next=%2Fcatalog",
                    "https://integration.example/woocommerce/callback?shop=merchant.example"
                ],
                "scope": ["product-listings:write", "users:read"]
            }),
            value
        );
        let keys = value
            .as_object()
            .unwrap_or_else(|| panic!("consent metadata must be an object"))
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            [
                "client_id",
                "client_name",
                "client_uri",
                "logo_uri",
                "policy_uri",
                "redirect_uris",
                "scope",
                "tos_uri",
            ],
            keys.as_slice()
        );
        assert!(!value.to_string().contains("client_secret"));
        assert!(!value.to_string().contains("secret_hash"));
        assert!(!value.to_string().contains("created"));
        assert!(!value.to_string().contains("updated"));
    }

    #[test]
    fn should_serialize_empty_allowed_scopes_as_an_empty_array() {
        let dto = OAuthClientConsentMetadataData::from(view(
            &["https://integration.example/callback"],
            HashSet::new(),
        ));
        let value = serde_json::to_value(dto)
            .unwrap_or_else(|error| panic!("failed to serialize consent metadata: {error}"));

        assert_eq!(serde_json::json!([]), value["scope"]);
    }

    #[tokio::test]
    async fn should_map_temporary_metadata_reader_failures_to_secret_free_no_store_503() {
        let error = oauth_service::error::OAuthServiceError::from(
            oauth_service::ports::OAuthClientReadError::TemporarilyUnavailable {
                source: application::error::box_error(std::io::Error::other(
                    "postgres password=must-not-escape",
                )),
            },
        );
        let response = no_store(ApiError::from(error).into_response());

        assert_eq!(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            response.status()
        );
        assert_eq!(
            Some("no-store"),
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok())
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("failed to read OAuth error body: {error}"));
        let body: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("failed to parse OAuth error body: {error}"));
        assert_eq!(503, body["status"]);
        assert_eq!("OAUTH_TEMPORARILY_UNAVAILABLE", body["error"]);
        assert!(!body.to_string().contains("postgres"));
        assert!(!body.to_string().contains("password"));
    }
}
