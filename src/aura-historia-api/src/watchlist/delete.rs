use crate::auth::protected_context;
use crate::error::{ApiError, INVALID_UUID};
use crate::state::WatchlistState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use product_listing_core::product_listing_id::ProductListingId;
use watchlist_service::use_cases::UnwatchProductListingCommand;

pub async fn delete_watchlist(
    State(state): State<WatchlistState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
) -> Response {
    let (ctx, user_id) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(v) => v,
        Err(r) => return *r,
    };
    let product_listing_id = match ProductListingId::try_from(raw_product_listing_id.as_str()) {
        Ok(v) => v,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productListingId")
                .with_detail("Path parameter 'productListingId' must be a product UUID.")
                .into_response();
        }
    };
    match state
        .unwatch_product
        .execute(
            &ctx,
            UnwatchProductListingCommand {
                user_id,
                product_listing_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => ApiError::from(e).into_response(),
    }
}
