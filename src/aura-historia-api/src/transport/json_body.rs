use crate::error::{ApiError, BAD_BODY_VALUE};
use axum::body::{Body, to_bytes};
use serde::de::DeserializeOwned;

/// Reads a JSON body of at most `max_bytes` for a handler that caps its body below the
/// transport-wide limit. An oversized, unreadable or malformed body is `400 BAD_BODY_VALUE`.
pub(crate) async fn parse_capped_json<T: DeserializeOwned>(
    body: Body,
    max_bytes: usize,
) -> Result<T, ApiError> {
    let bytes = to_bytes(body, max_bytes)
        .await
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))
}
