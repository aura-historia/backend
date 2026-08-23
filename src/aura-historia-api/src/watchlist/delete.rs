use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::WatchlistState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use product_core::product_id::ProductId;
use watchlist_service::use_cases::UnwatchProductCommand;

pub async fn delete_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_id): Path<String>,
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
    match state
        .unwatch_product
        .execute(
            &ctx,
            UnwatchProductCommand {
                user_id,
                product_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
