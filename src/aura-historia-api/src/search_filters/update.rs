use super::types::{SearchFilterData, UpdateSearchFilterData};
use super::util::{last_modified, no_store, parse_json, parse_search_filter_id};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use search_filter_service::use_cases::UpdateOwnedSearchFilterCommand;

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
    let (name, notifications, state_field, search) = match data.into_fields() {
        Ok(fields) => fields,
        Err(error) => return error.into_response(),
    };
    match state
        .update_owned_search_filter
        .execute(
            &context,
            UpdateOwnedSearchFilterCommand {
                user_id,
                search_filter_id,
                name,
                notifications,
                state: state_field,
                search,
            },
        )
        .await
    {
        Ok(result) => {
            let language = result.filter.search.language.as_str();
            let updated = result.filter.updated;
            let mut response = Json(SearchFilterData::from(result.filter)).into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_LANGUAGE,
                axum::http::HeaderValue::from_static(language),
            );
            last_modified(&mut response, updated);
            no_store(response)
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
