use super::create_client::OAuthClientMetadataData;
use super::{no_store, parse_scopes};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_service::use_cases::UpdateOAuthClientCommand;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct UpdateOAuthClientData {
    client_name: Option<String>,
    redirect_uris: Option<HashSet<url::Url>>,
    tos_uri: Option<url::Url>,
    policy_uri: Option<url::Url>,
    client_uri: Option<url::Url>,
    logo_uri: Option<url::Url>,
    scope: Option<HashSet<String>>,
}

impl TryFrom<UpdateOAuthClientData> for UpdateOAuthClientCommand {
    type Error = Response;
    fn try_from(data: UpdateOAuthClientData) -> Result<Self, Self::Error> {
        Ok(Self {
            name: data
                .client_name
                .map(oauth_core::client::OAuthClientName::from),
            redirect_uris: data.redirect_uris,
            tos_uri: data.tos_uri,
            policy_uri: data.policy_uri,
            client_uri: data.client_uri,
            logo_uri: data.logo_uri,
            scopes: data.scope.map(parse_scopes).transpose()?,
        })
    }
}

pub async fn update_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let client_id = match OAuthClientId::try_from(raw.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("clientId")
                .into_response();
        }
    };
    let data: UpdateOAuthClientData = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => {
            return ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    let command = match UpdateOAuthClientCommand::try_from(data) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .update_client
        .execute(&context, &client_id, command)
        .await
    {
        Ok(result) => no_store(Json(OAuthClientMetadataData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
