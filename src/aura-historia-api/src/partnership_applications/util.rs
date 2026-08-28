use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use serde::Deserialize;
use uuid::Uuid;

pub(super) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) fn parse_id(raw: &str) -> Result<PartnershipApplicationId, ApiError> {
    Uuid::parse_str(raw)
        .map(PartnershipApplicationId::from)
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("partnershipApplicationId")
                .with_detail("Path parameter 'partnershipApplicationId' must be a UUID.")
        })
}

pub(super) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Request body is required."));
    }
    serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}
