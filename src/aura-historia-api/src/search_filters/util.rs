use super::types::SearchFilterData;
use crate::error::{ApiError, BAD_BODY_VALUE, BAD_QUERY_PARAMETER_VALUE};
use crate::wire::parse_path_object_id;
use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::SearchFilterDetails;
use serde::{Deserialize, de::DeserializeOwned};
use time::OffsetDateTime;

pub(super) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// One search filter representation with its `Content-Language` and `Last-Modified` headers.
pub(super) fn search_filter_response(status: StatusCode, filter: SearchFilterDetails) -> Response {
    let language = filter.view.search.language.as_str();
    let updated = filter.view.updated;
    let mut response = (status, Json(SearchFilterData::from(filter))).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(language));
    last_modified(&mut response, updated);
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
    parse_path_object_id(raw, "userSearchFilterId", "UserSearchFilter")
        .map_err(IntoResponse::into_response)
}

#[allow(clippy::result_large_err)]
pub(super) fn parse_product_listing_id(raw: &str) -> Result<ProductListingId, Response> {
    parse_path_object_id(raw, "productListingId", "ProductListing")
        .map_err(IntoResponse::into_response)
}
