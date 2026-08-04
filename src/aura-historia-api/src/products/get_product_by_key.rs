use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, INVALID_UUID};
use crate::products::product_data::product_response;
use crate::state::ProductsState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use product_service::use_cases::GetProductRequest;

pub async fn get_product_by_key(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path((raw_shop_id, raw_shops_product_id)): Path<(String, String)>,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(principal) => principal,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let shop_id = match ShopId::try_from(raw_shop_id.as_str()) {
        Ok(shop_id) => shop_id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("shopId")
                .with_detail("Path parameter 'shopId' must be a UUID.")
                .into_response();
        }
    };

    let shops_product_id = match ShopsProductId::raw(&raw_shops_product_id) {
        Ok(shops_product_id) => shops_product_id,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopsProductId")
                .with_detail("Path parameter 'shopsProductId' is invalid.")
                .into_response();
        }
    };

    let context = principal.operation_context(metadata);
    match state
        .get_product
        .execute(
            &context,
            GetProductRequest::ByKey(ProductKey::new(shop_id, shops_product_id)),
        )
        .await
    {
        Ok(view) => product_response(view, &context.principal),
        Err(error) => ApiError::from(error).into_response(),
    }
}
