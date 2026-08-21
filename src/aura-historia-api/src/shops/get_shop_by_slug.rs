use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::ApiError;
use crate::shops::shop_data::{cache_control, shop_response};
use crate::state::ShopsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use shop_core::shop_slug_id::ShopSlugId;
use shop_service::use_cases::queries::get_shop::GetShopRequest;

pub async fn get_shop_by_slug(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    Path(raw_shop_slug_id): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let context = principal.operation_context(metadata);
    let cache_control = cache_control(&context.principal);
    match state
        .get_shop
        .execute(
            &context,
            GetShopRequest::BySlug(ShopSlugId::from(raw_shop_slug_id.as_str())),
        )
        .await
    {
        Ok(view) => shop_response(view, Some(cache_control)),
        Err(error) => ApiError::from(error).into_response(),
    }
}
