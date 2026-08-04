use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, BAD_PATH_PARAMETER_VALUE, INVALID_UUID, PRODUCT_INTERNAL_ERROR};
use crate::state::ProductsState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use common::product_id::ProductId;
use common::product_slug_id::ProductSlugId;
use common::shop_slug_id::ShopSlugId;
use product_service::ports::ProductEmbeddingLookup;
use product_service::use_cases::{GetSimilarProductsRequest, GetSimilarProductsResult};

const PENDING_CACHE_CONTROL: &str = "public, max-age=300, s-maxage=900";

pub async fn get_similar_products_by_id(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path(raw_product_id): Path<String>,
) -> Response {
    let product_id = match ProductId::try_from(raw_product_id.as_str()) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productId")
                .with_detail("Path parameter 'productId' must be a UUID.")
                .into_response();
        }
    };
    similar_response(state, headers, ProductEmbeddingLookup::ById(product_id)).await
}

pub async fn get_similar_products_by_slug(
    State(state): State<ProductsState>,
    headers: HeaderMap,
    Path((raw_shop_slug_id, raw_product_slug_id)): Path<(String, String)>,
) -> Response {
    let shop_slug_id = match ShopSlugId::raw(&raw_shop_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("shopSlugId")
                .with_detail("Path parameter 'shopSlugId' is invalid.")
                .into_response();
        }
    };
    let product_slug_id = match ProductSlugId::raw(&raw_product_slug_id) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_PATH_PARAMETER_VALUE)
                .with_path_field("productSlugId")
                .with_detail("Path parameter 'productSlugId' is invalid.")
                .into_response();
        }
    };
    similar_response(
        state,
        headers,
        ProductEmbeddingLookup::BySlug {
            shop_slug_id,
            product_slug_id,
        },
    )
    .await
}

async fn similar_response(
    state: ProductsState,
    headers: HeaderMap,
    lookup: ProductEmbeddingLookup,
) -> Response {
    let metadata = request_metadata(&headers);
    if let Err(error) = OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        return ApiError::from(error).into_response();
    }
    match state
        .get_similar_products
        .execute(GetSimilarProductsRequest {
            lookup: lookup.clone(),
        })
        .await
    {
        Ok(GetSimilarProductsResult::EmbeddingPending) => pending_response(lookup),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn pending_response(lookup: ProductEmbeddingLookup) -> Response {
    let location_path = match lookup {
        ProductEmbeddingLookup::ById(product_id) => {
            format!("/api/v1/products/{product_id}/similar")
        }
        ProductEmbeddingLookup::BySlug {
            shop_slug_id,
            product_slug_id,
        } => format!("/api/v1/by-slug/shops/{shop_slug_id}/products/{product_slug_id}/similar"),
    };
    let location = match HeaderValue::from_str(&location_path) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::internal_server_error(PRODUCT_INTERNAL_ERROR)
                .with_detail("Similar product polling location failed internally.")
                .into_response();
        }
    };
    let mut response = StatusCode::ACCEPTED.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(PENDING_CACHE_CONTROL),
    );
    response
}
