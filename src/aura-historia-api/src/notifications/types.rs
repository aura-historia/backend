use crate::error::{ApiError, BAD_BODY_VALUE};
pub(crate) use aura_historia_contracts::notifications::{
    UpdateNotificationSeenData, UpdateNotificationsSeenData,
};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

#[allow(clippy::result_large_err)]
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
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
