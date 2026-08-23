use super::create_client::OAuthClientMetadataData;
use super::no_store;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::OAuthState;
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};

pub async fn list_clients(State(state): State<OAuthState>, headers: HeaderMap) -> Response {
    let (_context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state.list_clients.execute().await {
        Ok(result) => no_store(
            Json(
                result
                    .into_iter()
                    .map(OAuthClientMetadataData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
