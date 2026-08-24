use crate::error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE, INVALID_UUID};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use product_listing_core::product_id::ProductId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, de::DeserializeOwned};
use time::OffsetDateTime;

pub(super) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn last_modified(response: &mut Response, updated: OffsetDateTime) {
    if let Ok(value) = HeaderValue::from_str(&httpdate::fmt_http_date(updated.into())) {
        response.headers_mut().insert(header::LAST_MODIFIED, value);
    }
}

#[allow(clippy::result_large_err)]
pub(super) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Request body is required.")
            .into_response());
    }
    serde_json::from_str(body).map_err(|error| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(error.to_string())
            .into_response()
    })
}

#[allow(clippy::result_large_err)]
pub(super) fn parse_json_query<T: DeserializeOwned>(
    raw: &str,
    field: &'static str,
) -> Result<T, ApiError> {
    serde_json::from_str(raw).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field(field)
            .with_detail(error.to_string())
    })
}

#[allow(clippy::result_large_err)]
pub(super) fn parse_search_filter_id(raw: &str) -> Result<UserSearchFilterId, Response> {
    UserSearchFilterId::try_from(raw).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("userSearchFilterId")
            .with_detail("Path parameter 'userSearchFilterId' must be a UUID.")
            .into_response()
    })
}

#[allow(clippy::result_large_err)]
pub(super) fn parse_product_id(raw: &str) -> Result<ProductId, Response> {
    ProductId::try_from(raw).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("productId")
            .with_detail("Path parameter 'productId' must be a UUID.")
            .into_response()
    })
}
