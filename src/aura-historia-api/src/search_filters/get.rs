use super::util::{no_store, parse_search_filter_id, search_filter_response};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use search_filter_service::use_cases::GetOwnedSearchFilterRequest;

pub(super) async fn get_search_filter(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path(raw_search_filter_id): Path<String>,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .get_owned_search_filter
        .execute(
            &context,
            GetOwnedSearchFilterRequest {
                user_id,
                search_filter_id,
            },
        )
        .await
    {
        Ok(result) => no_store(search_filter_response(StatusCode::OK, result.filter)),
        Err(error) => ApiError::from(error).into_response(),
    }
}
