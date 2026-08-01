use super::types::WatchlistEntryData;
use super::util::no_store;
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::WatchlistState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use watchlist_service::use_cases::ListWatchlistRequest;

pub async fn list_watchlist(State(state): State<WatchlistState>, headers: HeaderMap) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match state
        .list_watchlist
        .execute(&ctx, ListWatchlistRequest { user_id })
        .await
    {
        Ok(r) => no_store(
            Json(
                r.entries
                    .into_iter()
                    .map(WatchlistEntryData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response(),
        ),
        Err(e) => ApiError::from(e).into_response(),
    }
}
