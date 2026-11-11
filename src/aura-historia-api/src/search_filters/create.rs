use super::types::CreateSearchFilterData;
use super::util::search_filter_response;
use crate::auth::protected_context;
use crate::error::{ApiError, SEARCH_FILTER_INTERNAL_ERROR};
use crate::state::SearchFiltersState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

pub(super) async fn create_search_filter(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: CreateSearchFilterData = match super::util::parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(error) => return ApiError::from(error).into_response(),
    };
    match state.create_search_filter.execute(&context, command).await {
        Ok(result) => {
            let search_filter_id = result.filter.search_filter_id;
            let mut response = search_filter_response(StatusCode::CREATED, result.filter);
            let location = format!("/v1/me/search-filters/{search_filter_id}");
            match HeaderValue::from_str(&location) {
                Ok(value) => {
                    response.headers_mut().insert(header::LOCATION, value);
                }
                Err(_) => {
                    return ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                        .with_detail("Search filter location failed internally.")
                        .into_response();
                }
            }
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
