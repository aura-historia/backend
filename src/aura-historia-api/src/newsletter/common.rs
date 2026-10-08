use crate::error::{ApiError, BAD_BODY_VALUE};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde::de::DeserializeOwned;

pub(crate) const MAX_NEWSLETTER_BODY_BYTES: usize = 8 * 1024;

pub(crate) async fn parse_body<T: DeserializeOwned>(body: Body) -> Result<T, ApiError> {
    let bytes = to_bytes(body, MAX_NEWSLETTER_BODY_BYTES)
        .await
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))
}

pub(crate) fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
