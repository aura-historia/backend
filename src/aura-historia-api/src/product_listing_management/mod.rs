pub(crate) mod search;
pub(crate) mod types;

use crate::error::*;
use axum::{
    body::Bytes,
    extract::rejection::BytesRejection,
    http::{HeaderValue, header},
    response::Response,
};
use serde::de::DeserializeOwned;

pub(crate) fn parse_body<T: DeserializeOwned>(
    body: Result<Bytes, BytesRejection>,
) -> Result<T, ApiError> {
    let body = body.map_err(|_| ApiError::bad_request(BAD_BODY_VALUE))?;
    let mut deserializer = serde_json::Deserializer::from_slice(&body);
    let data = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let path = error.path().to_string();
        let mut response =
            ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.inner().to_string());
        // Keep the source field within the public whitelist, including nested facts.
        if let Some(field) = [
            "auction.timing.biddingOpens",
            "auction.timing.scheduledCloses",
            "auction.timing.reportedClosedAt",
            "auction.auctionId",
            "auction.lotNumber",
            "auction.cataloguePosition",
            "auction.timing",
            "expectedVersion",
            "priceEstimateMin",
            "priceEstimateMax",
            "price",
            "availability",
            "url",
            "images",
            "auction",
        ]
        .into_iter()
        .find(|field| {
            path == *field
                || path.starts_with(&format!("{field}."))
                || path.starts_with(&format!("{field}["))
        }) {
            response = response.with_body_field(field);
        }
        response
    })?;
    deserializer
        .end()
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    Ok(data)
}
pub(crate) async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}
