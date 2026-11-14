use crate::error::ApiError;
use crate::transport::json_body::parse_capped_json;
use axum::body::Body;
use serde::de::DeserializeOwned;

pub(crate) const MAX_NEWSLETTER_BODY_BYTES: usize = 8 * 1024;

pub(crate) async fn parse_body<T: DeserializeOwned>(body: Body) -> Result<T, ApiError> {
    parse_capped_json(body, MAX_NEWSLETTER_BODY_BYTES).await
}
