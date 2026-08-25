use super::types::{SearchFilterMatchData, UpdateSearchFilterMatchFeedbackData};
use super::util::{last_modified, no_store, parse_product_listing_id, parse_search_filter_id};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use search_filter_service::use_cases::UpdateSearchFilterMatchFeedbackCommand;

pub(super) async fn update_search_filter_match_feedback(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path((raw_search_filter_id, raw_product_listing_id)): Path<(String, String)>,
    body: String,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let product_listing_id = match parse_product_listing_id(&raw_product_listing_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: UpdateSearchFilterMatchFeedbackData = match super::util::parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match state
        .update_search_filter_match_feedback
        .execute(
            &context,
            UpdateSearchFilterMatchFeedbackCommand {
                user_id,
                search_filter_id,
                product_listing_id,
                feedback: data.feedback(),
            },
        )
        .await
    {
        Ok(result) => {
            let updated = result.search_filter_match.updated;
            let mut response =
                Json(SearchFilterMatchData::from(result.search_filter_match)).into_response();
            last_modified(&mut response, updated);
            no_store(response)
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
