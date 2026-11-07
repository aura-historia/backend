use super::{no_store, scope_strings};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, INVALID_CREDENTIALS};
use crate::state::OAuthState;
use application::operation_context::Principal;
use axum::{
    Json,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use oauth_service::use_cases::{
    GetMetadataDocumentConsentClientRequest, MetadataDocumentConsentClientView,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Deserialize)]
struct ConsentClientQuery {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    redirect_uri: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct MetadataDocumentConsentClientData {
    client_id: String,
    client_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_uri: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logo_uri: Option<Url>,
    client_host: String,
    redirect_uri_host: String,
    loopback_redirect_uris_only: bool,
    scope: Vec<String>,
    /// Scopes the user already granted this client.
    granted_scope: Vec<String>,
}

impl From<MetadataDocumentConsentClientView> for MetadataDocumentConsentClientData {
    fn from(view: MetadataDocumentConsentClientView) -> Self {
        Self {
            client_id: view.client_id.into(),
            client_name: view.name.into(),
            client_uri: view.client_uri,
            logo_uri: view.logo_uri,
            client_host: view.client_host,
            redirect_uri_host: view.redirect_uri_host,
            loopback_redirect_uris_only: view.loopback_redirect_uris_only,
            scope: scope_strings(view.scopes),
            granted_scope: scope_strings(view.granted_scopes),
        }
    }
}

/// Consent metadata for a client identified by its metadata document URL. The URL travels as
/// a query parameter, with the callback the authorization request names.
pub async fn get_metadata_document_consent_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    if !matches!(&context.principal, Principal::User(_)) {
        return no_store(ApiError::unauthorized(INVALID_CREDENTIALS).into_response());
    }
    let request = match request(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };
    match state
        .get_metadata_document_consent_client
        .execute(&context, request)
        .await
    {
        Ok(view) => no_store(Json(MetadataDocumentConsentClientData::from(view)).into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn request(raw_query: Option<&str>) -> Result<GetMetadataDocumentConsentClientRequest, ApiError> {
    let query: ConsentClientQuery = crate::wire::from_query_str(raw_query.unwrap_or_default())
        .map_err(|_| bad_query("query", "Query parameters are invalid."))?;
    let client_id = query
        .client_id
        .as_deref()
        .and_then(|value| MetadataDocumentClientId::try_from(value).ok())
        .ok_or_else(|| {
            bad_query(
                "client_id",
                "client_id must be a client metadata document URL; registered clients use /v1/oauth/clients/{clientId}.",
            )
        })?;
    let redirect_uri = query
        .redirect_uri
        .as_deref()
        .and_then(|value| Url::parse(value).ok())
        .ok_or_else(|| bad_query("redirect_uri", "redirect_uri must be an absolute URL."))?;
    Ok(GetMetadataDocumentConsentClientRequest {
        client_id,
        redirect_uri,
    })
}

fn bad_query(field: &'static str, detail: &'static str) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth_core::client::OAuthClientName;
    use serde_json::json;
    use std::collections::HashSet;
    use user_core::access_token::Scope;

    #[test]
    fn should_require_a_metadata_document_client_id_and_an_absolute_callback() {
        let request = request(Some(
            "client_id=https%3A%2F%2Fagent.example%2Fclient.json&redirect_uri=http%3A%2F%2F127.0.0.1%3A4000%2Fcallback",
        ))
        .unwrap();
        assert_eq!(
            "https://agent.example/client.json",
            request.client_id.as_str()
        );
        assert_eq!(
            "http://127.0.0.1:4000/callback",
            request.redirect_uri.as_str()
        );

        for (query, field) in [
            (
                "redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback",
                "client_id",
            ),
            (
                "client_id=oc_01h455vb4pex5vy7enb1p677vn&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb",
                "client_id",
            ),
            (
                "client_id=https%3A%2F%2Fagent.example&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb",
                "client_id",
            ),
            (
                "client_id=https%3A%2F%2Fagent.example%2Fclient.json",
                "redirect_uri",
            ),
            (
                "client_id=https%3A%2F%2Fagent.example%2Fclient.json&redirect_uri=%2Fcallback",
                "redirect_uri",
            ),
        ] {
            let error = super::request(Some(query)).unwrap_err();
            let body = serde_json::to_value(&error).unwrap();
            assert_eq!("BAD_QUERY_PARAMETER_VALUE", body["error"], "{query}");
            assert_eq!(
                json!({"field": field, "type": "QUERY"}),
                body["source"],
                "{query}"
            );
        }
    }

    #[test]
    fn should_serialize_verified_hosts_and_omit_absent_document_uris() {
        let value = serde_json::to_value(MetadataDocumentConsentClientData::from(
            MetadataDocumentConsentClientView {
                client_id: "https://agent.example/client.json".parse().unwrap(),
                name: OAuthClientName::from("Example Agent"),
                client_uri: Some(Url::parse("https://agent.example").unwrap()),
                logo_uri: None,
                client_host: "agent.example".to_owned(),
                redirect_uri_host: "127.0.0.1".to_owned(),
                loopback_redirect_uris_only: true,
                scopes: HashSet::from([Scope::UsersRead, Scope::WatchlistRead]),
                granted_scopes: HashSet::from([Scope::UsersRead]),
            },
        ))
        .unwrap();

        assert_eq!(
            json!({
                "client_id": "https://agent.example/client.json",
                "client_name": "Example Agent",
                "client_uri": "https://agent.example/",
                "client_host": "agent.example",
                "redirect_uri_host": "127.0.0.1",
                "loopback_redirect_uris_only": true,
                "scope": ["users:read", "watchlist:read"],
                "granted_scope": ["users:read"],
            }),
            value
        );
    }
}
