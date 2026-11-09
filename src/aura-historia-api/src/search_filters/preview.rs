use super::types::{PreviewSearchFilterMatchesData, SearchFilterMatchPreviewData};
use super::util::{no_store, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

pub(super) async fn preview_search_filter_matches(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: PreviewSearchFilterMatchesData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(error) => return ApiError::from(error).into_response(),
    };
    match state
        .preview_search_filter_matches
        .execute(&context, command)
        .await
    {
        Ok(preview) => no_store(Json(SearchFilterMatchPreviewData::from(preview)).into_response()),
        Err(error) => ApiError::from(error).into_response(),
    }
}
