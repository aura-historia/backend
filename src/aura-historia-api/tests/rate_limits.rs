//! The server's request-rate limits, through the real router and transport middleware.

use aura_historia_api::{
    RateLimit, app_with_rate_limits, state::AppState, transport::REQUEST_TIMEOUT,
};
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt;

/// One request per route class, refilled far slower than the test runs.
fn app() -> Router {
    app_with_rate_limits(
        AppState::new(),
        REQUEST_TIMEOUT,
        RateLimit::new(0.001, 1.0).unwrap(),
        RateLimit::new(0.001, 1.0).unwrap(),
    )
}

async fn get(app: &Router, path: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::get(path)
                .header("origin", "https://aura-historia.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn should_answer_an_exhausted_limit_with_a_429_that_browsers_can_read() {
    let app = app();
    assert_ne!(
        StatusCode::TOO_MANY_REQUESTS,
        get(&app, "/v1/ready").await.status()
    );

    let limited = get(&app, "/v1/ready").await;

    assert_eq!(StatusCode::TOO_MANY_REQUESTS, limited.status());
    let headers = limited.headers();
    assert_eq!("1", headers["retry-after"]);
    assert_eq!("*", headers["access-control-allow-origin"]);
    assert!(headers.contains_key("x-request-id"));
    let body = axum::body::to_bytes(limited.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!("REQUEST_RATE_LIMITED", body["error"]);
}

#[tokio::test]
async fn should_keep_answering_liveness_probes_once_the_limit_is_exhausted() {
    let app = app();
    get(&app, "/v1/ready").await;
    assert_eq!(
        StatusCode::TOO_MANY_REQUESTS,
        get(&app, "/v1/ready").await.status()
    );

    for _ in 0..3 {
        assert_eq!(StatusCode::OK, get(&app, "/v1/health").await.status());
    }
}

#[tokio::test]
async fn should_not_route_the_removed_gateway_stage_prefix() {
    let response = get(&app(), "/api/v1/health").await;

    assert_eq!(StatusCode::NOT_FOUND, response.status());
    assert!(!response.headers().contains_key("location"));
}
