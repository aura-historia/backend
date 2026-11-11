use super::types::UpdateSearchFilterData;
use super::util::{no_store, parse_json, parse_search_filter_id, search_filter_response};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub(super) async fn update_search_filter(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path(raw_search_filter_id): Path<String>,
    body: String,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: UpdateSearchFilterData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let command = match data.into_command(user_id, search_filter_id) {
        Ok(command) => command,
        Err(error) => return ApiError::from(error).into_response(),
    };
    match state
        .update_owned_search_filter
        .execute(&context, command)
        .await
    {
        Ok(result) => no_store(search_filter_response(StatusCode::OK, result.filter)),
        Err(error) => ApiError::from(error).into_response(),
    }
}
