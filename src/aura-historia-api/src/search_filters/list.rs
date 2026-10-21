use super::types::{PaginatedData, SearchFilterData};
use super::util::no_store;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use search_filter_service::use_cases::ListOwnedSearchFiltersRequest;

pub(super) async fn list_search_filters(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
) -> Response {
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .list_owned_search_filters
        .execute(&context, ListOwnedSearchFiltersRequest { user_id })
        .await
    {
        Ok(result) => {
            no_store(Json(PaginatedData::<SearchFilterData>::from(result.items)).into_response())
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
