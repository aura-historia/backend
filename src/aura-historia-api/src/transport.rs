use crate::auth::RequestMetadata;
use crate::error::{ApiError, BAD_BODY_VALUE};
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use product_listing_ingestion_sqs::with_publication_deadline;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub(crate) const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");
pub(crate) const CORRELATION_ID_HEADER: HeaderName = HeaderName::from_static("x-correlation-id");

const WOOCOMMERCE_DELIVERY_ID_HEADER: HeaderName =
    HeaderName::from_static("x-wc-webhook-delivery-id");
const IDEMPOTENCY_KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");
const MAX_CORRELATION_ID_LENGTH: usize = 128;
const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;
/// Leave time to map and serialize a bounded publisher outcome before the HTTP timeout.
pub(crate) const PUBLICATION_REPORT_HEADROOM: Duration = Duration::from_millis(300);
pub(crate) const NATIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const LAMBDA_REQUEST_TIMEOUT: Duration = Duration::from_secs(14);

pub(crate) fn with_transport_middleware(router: Router, request_timeout: Duration) -> Router {
    router
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                tracing::info_span!(
                    "http.request",
                    method = %request.method(),
                    path = %safe_request_path(request.uri().path()),
                    request_id = ?request.headers().get(&REQUEST_ID_HEADER),
                    correlation_id = ?request.headers().get(&CORRELATION_ID_HEADER),
                )
            }),
        )
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
            HeaderName::from_static("x-api-key"),
            HeaderName::from_static("x-wc-webhook-signature"),
            IDEMPOTENCY_KEY_HEADER,
        ]))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BODY_BYTES))
        .layer(axum::middleware::from_fn(async_ingestion_body_limit_error))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PATCH,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    HeaderName::from_static("x-wc-webhook-topic"),
                    HeaderName::from_static("x-wc-webhook-signature"),
                    WOOCOMMERCE_DELIVERY_ID_HEADER,
                    CORRELATION_ID_HEADER,
                    IDEMPOTENCY_KEY_HEADER,
                ])
                .expose_headers([
                    REQUEST_ID_HEADER,
                    CORRELATION_ID_HEADER,
                    IDEMPOTENCY_KEY_HEADER,
                ]),
        )
        .layer(axum::middleware::from_fn(request_metadata))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: Next| async move {
                let deadline = tokio::time::Instant::now()
                    + request_timeout.saturating_sub(PUBLICATION_REPORT_HEADROOM);
                with_publication_deadline(deadline, next.run(request)).await
            },
        ))
}

async fn async_ingestion_body_limit_error(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let async_ingestion = matches!(
        request.method(),
        &Method::POST | &Method::PATCH | &Method::PUT | &Method::DELETE
    ) && path.starts_with("/api/v1/listing-sources/")
        && path.ends_with("/product-listings/async");
    let woocommerce_webhook = request.method() == Method::POST
        && path
            .strip_prefix("/api/v1/webhooks/woocommerce/")
            .is_some_and(|source| !source.is_empty() && !source.contains('/'));
    let response = next.run(request).await;
    if (async_ingestion || woocommerce_webhook)
        && response.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value == "text/plain; charset=utf-8")
    {
        ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "Payload Too Large",
            BAD_BODY_VALUE,
        )
        .into_response()
    } else {
        response
    }
}

fn safe_request_path(path: &str) -> &str {
    path.strip_prefix("/api/v1/listing-sources/by-slug/")
        .map(|_| "/api/v1/listing-sources/by-slug/{listingSourceSlugId}")
        .unwrap_or(path)
}

async fn request_metadata(mut request: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let correlation_id = request
        .headers()
        .get(&CORRELATION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_correlation_id(value))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| request_id.clone());
    let metadata = RequestMetadata::new(request_id.clone(), correlation_id.clone());

    // Route extractors receive the same immutable metadata through the headers and extensions.
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        request.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        request.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    request.extensions_mut().insert(metadata);

    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    response
}

fn valid_correlation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CORRELATION_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, Bytes, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn app() -> Router {
        with_transport_middleware(
            Router::new().route("/", get(|| async { "ok" })),
            NATIVE_REQUEST_TIMEOUT,
        )
    }

    #[tokio::test]
    async fn should_return_api_error_for_oversized_woocommerce_body() {
        let app = with_transport_middleware(
            Router::new().route(
                "/api/v1/webhooks/woocommerce/{source}",
                post(|_: Bytes| async { StatusCode::NO_CONTENT }),
            ),
            NATIVE_REQUEST_TIMEOUT,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/webhooks/woocommerce/ls_test")
                    .body(Body::from(vec![b'a'; MAX_REQUEST_BODY_BYTES + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(StatusCode::PAYLOAD_TOO_LARGE, response.status());
        assert_eq!(
            "application/problem+json",
            response.headers()[header::CONTENT_TYPE]
        );
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!("BAD_BODY_VALUE", body["error"]);
    }

    #[test]
    fn should_redact_public_listing_source_slug_from_request_trace_path() {
        assert_eq!(
            "/api/v1/listing-sources/by-slug/{listingSourceSlugId}",
            safe_request_path("/api/v1/listing-sources/by-slug/private-source-slug"),
        );
        assert_eq!("/api/v1/health", safe_request_path("/api/v1/health"));
    }

    #[tokio::test]
    async fn should_generate_and_propagate_bounded_request_metadata() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
        let request_id = response.headers()[&REQUEST_ID_HEADER].to_str().unwrap();
        assert!(uuid::Uuid::parse_str(request_id).is_ok());
        assert_eq!(request_id, response.headers()[&CORRELATION_ID_HEADER]);
    }

    #[tokio::test]
    async fn should_preserve_only_valid_bounded_correlation_ids() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(CORRELATION_ID_HEADER, "trace_123-abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!("trace_123-abc", response.headers()[&CORRELATION_ID_HEADER]);
    }

    #[tokio::test]
    async fn should_replace_invalid_correlation_ids() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(CORRELATION_ID_HEADER, "contains space")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers()[&REQUEST_ID_HEADER],
            response.headers()[&CORRELATION_ID_HEADER]
        );
    }

    #[tokio::test]
    async fn should_expose_correlation_headers_for_cors_requests() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!("*", response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN]);
        assert!(
            response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .contains(REQUEST_ID_HEADER.as_str())
        );
    }

    #[tokio::test]
    async fn should_allow_woocommerce_delivery_id_for_cors_preflight() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, Method::POST.as_str())
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        WOOCOMMERCE_DELIVERY_ID_HEADER.as_str(),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
        let allowed_headers = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(allowed_headers.split(',').any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case(WOOCOMMERCE_DELIVERY_ID_HEADER.as_str())
        }));
    }

    #[tokio::test]
    async fn should_allow_and_expose_idempotency_key_for_cors_requests() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, Method::POST.as_str())
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        IDEMPOTENCY_KEY_HEADER.as_str(),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
        assert!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap()
                .split(',')
                .any(|value| value
                    .trim()
                    .eq_ignore_ascii_case(IDEMPOTENCY_KEY_HEADER.as_str()))
        );

        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .split(',')
                .any(|value| value
                    .trim()
                    .eq_ignore_ascii_case(IDEMPOTENCY_KEY_HEADER.as_str()))
        );
    }

    #[tokio::test]
    async fn should_reject_requests_above_the_body_limit() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .header(header::CONTENT_LENGTH, MAX_REQUEST_BODY_BYTES + 1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(axum::http::StatusCode::PAYLOAD_TOO_LARGE, response.status());
    }
}
