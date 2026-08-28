use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, INVALID_UUID};
use crate::state::WebhooksState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use product_listing_service::use_cases::{
    IngestWoocommerceProductListingCommand, WoocommerceProductEventKind,
};
use serde::Deserialize;
use url::Url;

const TOPIC_HEADER: &str = "x-wc-webhook-topic";
const SIGNATURE_HEADER: &str = "x-wc-webhook-signature";

#[derive(Debug, Deserialize)]
struct WoocommerceProductDto {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    permalink: Option<Url>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    price: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    stock_status: Option<String>,
    #[serde(default)]
    images: Vec<WoocommerceImageDto>,
}

#[derive(Debug, Deserialize)]
struct WoocommerceImageDto {
    src: Url,
}

pub async fn post_woocommerce(
    State(state): State<WebhooksState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: Bytes,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    if body.is_empty() {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Body cannot be empty.")
            .into_response();
    }
    let kind = match event_kind(&headers) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let signature = match signature(&headers) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let payload = match serde_json::from_slice::<WoocommerceProductDto>(&body) {
        Ok(value) => value,
        Err(_) => {
            return ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("Body must contain a valid WooCommerce product JSON value.")
                .into_response();
        }
    };
    let source_listing_id = match product_listing_core::source_listing_id::SourceListingId::try_from(
        payload.id.to_string(),
    ) {
        Ok(value) => value,
        Err(error) => {
            return ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .ingest
        .execute(
            &context,
            IngestWoocommerceProductListingCommand {
                listing_source_id,
                kind,
                signature,
                raw_body: body.to_vec(),
                source_listing_id,
                title: payload.name,
                permalink: payload.permalink,
                description_html: payload.description,
                short_description_html: payload.short_description,
                price: payload.price,
                status: payload.status,
                stock_status: payload.stock_status,
                image_urls: payload
                    .images
                    .into_iter()
                    .map(|image| image.src)
                    .collect::<IndexSet<_>>(),
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    uuid::Uuid::parse_str(value)
        .map(ListingSourceId::from)
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("listingSourceId")
                .with_detail("Path parameter 'listingSourceId' must be a UUID.")
        })
}

fn event_kind(headers: &HeaderMap) -> Result<WoocommerceProductEventKind, ApiError> {
    match headers
        .get(TOPIC_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some("product.created") => Ok(WoocommerceProductEventKind::Create),
        Some("product.updated") => Ok(WoocommerceProductEventKind::Update),
        Some("product.deleted") => Ok(WoocommerceProductEventKind::Delete),
        Some(_) => Err(ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field(TOPIC_HEADER)
            .with_detail("WooCommerce topic is unsupported.")),
        None => Err(ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field(TOPIC_HEADER)
            .with_detail("WooCommerce topic header is required.")),
    }
}

fn signature(headers: &HeaderMap) -> Result<Vec<u8>, ApiError> {
    let encoded = headers
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(SIGNATURE_HEADER)
                .with_detail("WooCommerce signature header is required.")
        })?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(SIGNATURE_HEADER)
                .with_detail("WooCommerce signature must be base64 encoded.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn should_parse_listing_source_id() {
        let listing_source_id = ListingSourceId::new();

        let parsed = parse_listing_source_id(&listing_source_id.to_string());

        assert!(matches!(parsed, Ok(actual) if actual == listing_source_id));
    }

    #[test]
    fn should_reject_invalid_listing_source_id() {
        let error = parse_listing_source_id("not-a-uuid");

        assert!(matches!(error, Err(error) if error.code() == INVALID_UUID));
    }

    #[test]
    fn should_map_supported_woocommerce_topics() {
        for (topic, expected) in [
            ("product.created", WoocommerceProductEventKind::Create),
            ("product.updated", WoocommerceProductEventKind::Update),
            ("product.deleted", WoocommerceProductEventKind::Delete),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(TOPIC_HEADER, HeaderValue::from_static(topic));
            assert!(matches!(event_kind(&headers), Ok(actual) if actual == expected));
        }
    }

    #[test]
    fn should_reject_missing_or_unsupported_woocommerce_topic() {
        let missing = event_kind(&HeaderMap::new());
        assert!(matches!(missing, Err(error) if error.code() == BAD_HEADER_VALUE));

        let mut headers = HeaderMap::new();
        headers.insert(TOPIC_HEADER, HeaderValue::from_static("order.created"));
        let unsupported = event_kind(&headers);
        assert!(matches!(unsupported, Err(error) if error.code() == BAD_HEADER_VALUE));
    }

    #[test]
    fn should_decode_valid_woocommerce_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("c2lnbmF0dXJl"));

        assert!(matches!(signature(&headers), Ok(value) if value == b"signature"));
    }

    #[test]
    fn should_reject_missing_or_invalid_woocommerce_signature() {
        let missing = signature(&HeaderMap::new());
        assert!(matches!(missing, Err(error) if error.code() == BAD_HEADER_VALUE));

        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("not-base64!"));
        let invalid = signature(&headers);
        assert!(matches!(invalid, Err(error) if error.code() == BAD_HEADER_VALUE));
    }
}
