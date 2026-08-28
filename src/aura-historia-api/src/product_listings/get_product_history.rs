use crate::auth::{OptionalAuthExtractor, request_metadata};
use crate::error::{ApiError, INVALID_UUID, PRODUCT_LISTING_INTERNAL_ERROR};
use crate::product_listings::product_event_data::ProductListingEventData;
use crate::state::ProductListingsState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};

use product_listing_core::product_listing_id::ProductListingId;

use product_listing_service::use_cases::{
    GetProductListingEventsRequest, ProductListingEventLookup,
};

const HISTORY_CACHE_CONTROL: &str = "public, max-age=180, s-maxage=900";

pub async fn get_product_listing_events_by_id(
    State(state): State<ProductListingsState>,
    headers: HeaderMap,
    Path(raw_product_listing_id): Path<String>,
) -> Response {
    let product_listing_id = match ProductListingId::try_from(raw_product_listing_id.as_str()) {
        Ok(id) => id,
        Err(_) => {
            return ApiError::bad_request(INVALID_UUID)
                .with_path_field("productListingId")
                .with_detail("Path parameter 'productListingId' must be a UUID.")
                .into_response();
        }
    };
    history_response(
        state,
        headers,
        ProductListingEventLookup::ById(product_listing_id),
    )
    .await
}

async fn history_response(
    state: ProductListingsState,
    headers: HeaderMap,
    lookup: ProductListingEventLookup,
) -> Response {
    let metadata = request_metadata(&headers);
    let principal = match OptionalAuthExtractor::new(state.authenticator.as_ref())
        .extract(&headers, &metadata)
        .await
    {
        Ok(value) => value,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let Some(use_case) = state.get_product_listing_events.as_ref() else {
        return ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
            .with_detail("ProductListing events are not configured.")
            .into_response();
    };
    let context = principal.operation_context(metadata);
    match use_case
        .execute(&context, GetProductListingEventsRequest { lookup })
        .await
    {
        Ok(events) => {
            let mut response = Json(
                events
                    .into_iter()
                    .map(ProductListingEventData::from)
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
