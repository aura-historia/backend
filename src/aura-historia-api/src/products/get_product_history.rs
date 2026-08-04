use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, INVALID_UUID, PRODUCT_INTERNAL_ERROR};
use crate::products::product_history_data::ProductHistoryEventData;
use crate::state::ProductsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use product_service::use_cases::GetProductHistoryRequest;

const HISTORY_CACHE_CONTROL: &str = "public, max-age=180, s-maxage=900";

pub async fn get_product_history(
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
    let Some(use_case) = state.get_product_history.as_ref() else {
        return ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
            .with_detail("Product history is not configured.")
            .into_response();
    };

    let context = principal.operation_context(metadata);
    match use_case
        .execute(
            &context,
            GetProductHistoryRequest {
                product_key: ProductKey::new(shop_id, shops_product_id),
            },
        )
        .await
    {
        Ok(events) => {
            let mut response = Json(
                events
                    .into_iter()
                    .map(ProductHistoryEventData::from)
                    .collect::<Vec<_>>(),
            )
            .into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static(HISTORY_CACHE_CONTROL),
            );
            response
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}
