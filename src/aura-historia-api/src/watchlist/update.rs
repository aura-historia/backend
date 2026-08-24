use super::types::{PatchWatchlistData, WatchlistEntryData, watchlist_state};
use super::util::parse_json;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::patch_value::non_nullable_option;
use crate::state::WatchlistState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use product_listing_core::product_id::ProductId;
use watchlist_service::use_cases::UpdateWatchlistProductCommand;

pub async fn patch_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_id): Path<String>,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let product_id = match ProductId::try_from(raw_product_id.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productId")
                .with_detail("Path parameter 'productId' must be a product UUID.")
                .into_response();
        }
    };
    let data: PatchWatchlistData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let notifications = match non_nullable_option(data.notifications, "notifications") {
        Ok(notifications) => notifications,
        Err(error) => return error.into_response(),
    };
    let state_field = match non_nullable_option(data.state, "state") {
        Ok(state_field) => state_field.map(watchlist_state),
        Err(error) => return error.into_response(),
    };
    match state
        .update_watchlist_product
        .execute(
            &ctx,
            UpdateWatchlistProductCommand {
                user_id,
                product_id,
                notifications,
                state: state_field,
            },
        )
        .await
    {
        Ok(r) => Json(WatchlistEntryData::from(r.entry)).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
