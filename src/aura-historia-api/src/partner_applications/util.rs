use crate::error::{ApiError, BAD_BODY_VALUE, INVALID_UUID};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use shop_partner_core::partner_shop_application_id::PartnerShopApplicationId;

pub(crate) fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}
#[allow(clippy::result_large_err)]
pub(crate) fn parse_id(raw: &str) -> Result<PartnerShopApplicationId, Response> {
    PartnerShopApplicationId::try_from(raw).map_err(|_| {
        ApiError::bad_request(INVALID_UUID)
            .with_path_field("partnerApplicationId")
            .with_detail("Path parameter 'partnerApplicationId' must be a UUID.")
            .into_response()
    })
}
#[allow(clippy::result_large_err)]
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Request body is required.")
            .into_response());
    }
    serde_json::from_str(body).map_err(|e| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(e.to_string())
            .into_response()
    })
}
