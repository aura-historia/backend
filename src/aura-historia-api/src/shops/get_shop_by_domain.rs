use crate::auth::OptionalAuthExtractor;
use crate::error::{ApiError, INVALID_DOMAIN};
use crate::shops::shop_data::{cache_control, request_metadata, shop_response};
use crate::state::ShopsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::domain::Domain;
use shop_service::use_cases::queries::get_shop::GetShopRequest;

pub async fn get_shop_by_domain(
    State(state): State<ShopsState>,
    headers: HeaderMap,
    Path(raw_domain): Path<String>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let domain = match Domain::try_from(raw_domain.as_str()) {
        Ok(domain) => domain,
        Err(_) => {
            return ApiError::bad_request(INVALID_DOMAIN)
                .with_path_field("shopDomain")
                .with_detail("Path parameter 'shopDomain' must be a valid domain.")
                .into_response();
        }
    };
    let context = principal.operation_context(metadata);
    let cache_control = cache_control(&context.principal);
    match state
        .get_shop
        .execute(&context, GetShopRequest::ByShopifyDomain(domain))
        .await
    {
        Ok(view) => shop_response(view, Some(cache_control)),
        Err(error) => ApiError::from(error).into_response(),
    }
}
