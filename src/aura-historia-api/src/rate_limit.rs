//! Request-rate limits of one API process, replacing the API Gateway stage throttle and the lower
//! route throttle of address suggestions. They shape bursts; per-user quotas stay with the use
//! cases. The [front door](../../../docs/http-api-front-door.md#front-door-controls) owns the
//! contract.

use crate::error::{ApiError, REQUEST_RATE_LIMITED};
use axum::{
    extract::{Request, State},
    http::Method,
    middleware::Next,
    response::{IntoResponse, Response},
};
use platform_process::{RateLimit, TokenBucket};
use std::sync::Arc;

const ADDRESS_SUGGESTIONS_PATH: &str = "/v1/address-suggestions";
/// The liveness probe must answer under load, or an overloaded replica is restarted. Readiness
/// stays limited: it queries PostgreSQL and OpenSearch.
const LIVENESS_PATH: &str = "/v1/health";
/// Cognito's triggers cannot retry, so a public flood must not fail sign-ups. Each request is
/// authenticated by its signature, over a small body, before it reaches the database.
const COGNITO_TRIGGER_PREFIX: &str = "/internal/cognito/";

/// The process's limits: one for every request and a lower one for address suggestions.
#[derive(Clone)]
pub(crate) struct RateLimits {
    every_request: Arc<TokenBucket>,
    address_suggestions: Arc<TokenBucket>,
}

impl RateLimits {
    pub(crate) fn new(every_request: RateLimit, address_suggestions: RateLimit) -> Self {
        Self {
            every_request: Arc::new(TokenBucket::new(every_request)),
            address_suggestions: Arc::new(TokenBucket::new(address_suggestions)),
        }
    }

    fn admits(&self, request: &Request) -> bool {
        let path = request.uri().path();
        // CORS preflights are answered by the transport middleware without reaching a route.
        if path == LIVENESS_PATH
            || path.starts_with(COGNITO_TRIGGER_PREFIX)
            || request.method() == Method::OPTIONS
        {
            return true;
        }
        if !self.every_request.try_acquire() {
            return false;
        }
        !(request.method() == Method::POST && path == ADDRESS_SUGGESTIONS_PATH)
            || self.address_suggestions.try_acquire()
    }
}

/// Answers `429 REQUEST_RATE_LIMITED` with `Retry-After: 1` once a limit is exhausted, before
/// authentication or any use case runs.
pub(crate) async fn enforce(
    State(limits): State<RateLimits>,
    request: Request,
    next: Next,
) -> Response {
    if limits.admits(&request) {
        next.run(request).await
    } else {
        ApiError::too_many_requests(REQUEST_RATE_LIMITED, 1).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::StatusCode, routing::get};
    use tower::ServiceExt;

    fn limit(per_second: f64, burst: f64) -> RateLimit {
        RateLimit::new(per_second, burst).unwrap()
    }

    fn app(limits: RateLimits) -> Router {
        Router::new()
            .route("/v1/health", get(|| async { "ok" }))
            .route(
                "/internal/cognito/post-confirmation",
                axum::routing::post(|| async { "ok" }),
            )
            .route("/v1/product-listings", get(|| async { "ok" }))
            .route(
                ADDRESS_SUGGESTIONS_PATH,
                axum::routing::post(|| async { "ok" }),
            )
            .layer(axum::middleware::from_fn_with_state(limits, enforce))
    }

    async fn status(app: &Router, method: Method, path: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn should_limit_address_suggestions_below_the_limit_of_every_request() {
        let app = app(RateLimits::new(limit(0.001, 3.0), limit(0.001, 1.0)));

        assert_eq!(
            StatusCode::OK,
            status(&app, Method::POST, ADDRESS_SUGGESTIONS_PATH).await
        );
        assert_eq!(
            StatusCode::TOO_MANY_REQUESTS,
            status(&app, Method::POST, ADDRESS_SUGGESTIONS_PATH).await
        );
        assert_eq!(
            StatusCode::OK,
            status(&app, Method::GET, "/v1/product-listings").await,
            "other routes keep their share of the overall limit"
        );
        let limited = app
            .clone()
            .oneshot(
                Request::get("/v1/product-listings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(StatusCode::TOO_MANY_REQUESTS, limited.status());
        assert_eq!("1", limited.headers()["retry-after"]);
        let body = axum::body::to_bytes(limited.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!("REQUEST_RATE_LIMITED", body["error"]);
        assert_eq!(
            StatusCode::OK,
            status(&app, Method::GET, "/v1/health").await
        );
        assert_eq!(
            StatusCode::OK,
            status(&app, Method::POST, "/internal/cognito/post-confirmation").await,
            "Cognito's triggers stay admitted while the public limit is exhausted"
        );
    }
}
