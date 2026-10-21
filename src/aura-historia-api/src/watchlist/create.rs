use super::types::{PostWatchlistData, WatchlistEntryData};
use super::util::parse_json;
use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_OBJECT_ID};
use crate::state::WatchlistState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

pub async fn post_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let data: PostWatchlistData = match parse_json(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let command = match data.into_command(user_id) {
        Ok(command) => command,
        Err(_) => {
            return ApiError::bad_request(INVALID_OBJECT_ID)
                .with_body_field("productListingId")
                .with_detail("must be a valid ProductListing ID")
                .into_response();
        }
    };
    match state.watch_product.execute(&ctx, command).await {
        Ok(r) => (StatusCode::CREATED, Json(WatchlistEntryData::from(r.entry))).into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
