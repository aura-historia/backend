use super::create_client::OAuthClientMetadataData;
use super::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::OAuthState;
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use credential_core::oauth_client_id::OAuthClientId;

pub async fn get_client(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> Response {
    let (_context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
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
    match state.get_client.execute(&client_id).await {
        Ok(result) => no_store(Json(OAuthClientMetadataData::from(result)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
