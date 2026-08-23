use super::{no_store, parse_scopes, scope_strings};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use oauth_service::use_cases::{CreateOAuthClientCommand, CreateOAuthClientResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CreateOAuthClientData {
    client_name: String,
    tos_uri: url::Url,
    policy_uri: url::Url,
    client_uri: url::Url,
    logo_uri: url::Url,
    #[serde(default)]
    redirect_uris: HashSet<url::Url>,
    #[serde(default)]
    scope: HashSet<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OAuthClientMetadataData {
    client_id: credential_core::oauth_client_id::OAuthClientId,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    client_name: String,
    tos_uri: url::Url,
    policy_uri: url::Url,
    client_uri: url::Url,
    logo_uri: url::Url,
    redirect_uris: HashSet<url::Url>,
    scope: Vec<String>,
    client_id_issued_at: i64,
}

impl TryFrom<CreateOAuthClientData> for CreateOAuthClientCommand {
    type Error = Response;
    fn try_from(data: CreateOAuthClientData) -> Result<Self, Self::Error> {
        Ok(Self {
            name: oauth_core::client::OAuthClientName::from(data.client_name),
            redirect_uris: data.redirect_uris,
            tos_uri: data.tos_uri,
            policy_uri: data.policy_uri,
            client_uri: data.client_uri,
            logo_uri: data.logo_uri,
            scopes: parse_scopes(data.scope)?,
        })
    }
}

impl From<oauth_core::client::OAuthClient> for OAuthClientMetadataData {
    fn from(client: oauth_core::client::OAuthClient) -> Self {
        Self {
            client_id: client.client_id,
            client_secret: None,
            client_name: client.name.into(),
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            redirect_uris: client.redirect_uris,
            scope: scope_strings(client.scopes),
            client_id_issued_at: client.created.unix_timestamp(),
        }
    }
}

impl From<CreateOAuthClientResult> for OAuthClientMetadataData {
    fn from(result: CreateOAuthClientResult) -> Self {
        let mut data = Self::from(result.client);
        data.client_secret = Some(result.raw_client_secret.into());
        data
    }
}

pub async fn create_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: CreateOAuthClientData = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return ApiError::bad_request(crate::error::BAD_BODY_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    let command = match CreateOAuthClientCommand::try_from(data) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state.create_client.execute(&context, command).await {
        Ok(result) => no_store(
            (
                StatusCode::CREATED,
                Json(OAuthClientMetadataData::from(result)),
            )
                .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
