use super::types::{CreateProductData, PartnerProductFailureData};
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use crate::state::PartnerProductsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use common::shop_id::ShopId;

pub async fn create_products(
    State(state): State<PartnerProductsState>,
    headers: HeaderMap,
    Path(raw_shop_id): Path<String>,
    body: String,
) -> Response {
    let shop_id = match parse_shop_id(&raw_shop_id) {
        Ok(shop_id) => shop_id,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let products: Vec<CreateProductData> = match parse_body(&body) {
        Ok(products) => products,
        Err(error) => return error.into_response(),
    };

    let mut failures = Vec::new();
    let mut first_error = None;
    let mut successes = 0;
    for product in products {
        let shops_product_id = product.shops_product_id.clone();
        match state
            .create
            .execute(&context, product.into_command(shop_id))
            .await
        {
            Ok(_) => successes += 1,
            Err(error) => {
                first_error.get_or_insert_with(|| ApiError::from(error));
                failures.push(PartnerProductFailureData::new(shop_id, shops_product_id));
            }
        }
    }

    if successes == 0
        && let Some(error) = first_error
    {
        return error.into_response();
    }

    (StatusCode::OK, Json(failures)).into_response()
}

fn parse_shop_id(value: &str) -> Result<ShopId, ApiError> {
    ShopId::try_from(value).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("shopId")
            .with_detail("Path parameter 'shopId' must be a UUID.")
    })
}

fn parse_body(body: &str) -> Result<Vec<CreateProductData>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
