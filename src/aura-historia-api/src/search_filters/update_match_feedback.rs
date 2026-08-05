use super::types::{SearchFilterMatchData, UpdateSearchFilterMatchFeedbackData};
use super::util::{
    last_modified, no_store, parse_search_filter_id, parse_shop_id, parse_shops_product_id,
};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::SearchFiltersState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::product_id::ProductKey;
use search_filter_service::use_cases::UpdateSearchFilterMatchFeedbackCommand;

pub(super) async fn update_search_filter_match_feedback(
    State(state): State<SearchFiltersState>,
    headers: HeaderMap,
    Path((raw_search_filter_id, raw_shop_id, raw_shops_product_id)): Path<(String, String, String)>,
    body: String,
) -> Response {
    let search_filter_id = match parse_search_filter_id(&raw_search_filter_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let shop_id = match parse_shop_id(&raw_shop_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let shops_product_id = match parse_shops_product_id(&raw_shops_product_id) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (context, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
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
                product_key: ProductKey::new(shop_id, shops_product_id),
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
