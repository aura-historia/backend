use axum::{
    Router,
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use lambda_http::RequestExt;
use platform_lambda_bootstrap::{LambdaInvocationBudget, log_invocation_start};
use std::{future::Future, time::Duration};
use tower::ServiceExt;

const COMPONENT: &str = "aura-historia-api";
const LAMBDA_RESPONSE_HEADROOM: Duration = Duration::from_secs(1);

/// Refreshes the caller-owned full router composition before each Lambda invocation.
pub async fn run<F, Fut>(router_for_invocation: F) -> Result<(), lambda_http::Error>
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Router, String>> + Send,
{
    lambda_http::run(lambda_http::service_fn(
        move |request: lambda_http::Request| {
            let router_for_invocation = router_for_invocation.clone();
            async move {
                let budget = request.lambda_context_ref().map(|context| {
                    LambdaInvocationBudget::from_context(
                        context,
                        crate::transport::LAMBDA_REQUEST_TIMEOUT,
                        LAMBDA_RESPONSE_HEADROOM,
                    )
                });
                let app = match compose_router_with_budget(
                    budget.as_ref().map(LambdaInvocationBudget::remaining),
                    router_for_invocation,
                )
                .await?
                {
                    Some(app) => app,
                    None => return Ok(request_timeout_response()),
                };
                handle_request(app, request)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
    ))
    .await
}

/// Returns `None` only when the invocation has no time left for credential refresh and
/// composition. Provider failures remain failures rather than silently using an old router.
async fn compose_router_with_budget<F, Fut>(
    budget: Option<Duration>,
    compose: F,
) -> Result<Option<Router>, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Router, String>>,
{
    match budget {
        Some(budget) if budget.is_zero() => Ok(None),
        Some(budget) => match tokio::time::timeout(budget, compose()).await {
            Ok(result) => result.map(Some),
            Err(_) => Ok(None),
        },
        None => compose().await.map(Some),
    }
}

async fn handle_request(
    app: Router,
    request: lambda_http::Request,
) -> Result<Response, LambdaHttpAdapterError> {
    let budget = request.lambda_context_ref().map(|context| {
        log_invocation_start(COMPONENT, context);
        LambdaInvocationBudget::from_context(
            context,
            crate::transport::LAMBDA_REQUEST_TIMEOUT,
            LAMBDA_RESPONSE_HEADROOM,
        )
    });
    let request = axum_request(request)?;

    Ok(match budget {
        Some(budget) => response_with_budget(app, request, budget.remaining()).await,
        None => response_from_app(app, request).await,
    })
}

async fn response_with_budget(app: Router, request: Request, budget: Duration) -> Response {
    if budget.is_zero() {
        return request_timeout_response();
    }

    match tokio::time::timeout(budget, response_from_app(app, request)).await {
        Ok(response) => response,
        Err(_) => request_timeout_response(),
    }
}

async fn response_from_app(app: Router, request: Request) -> Response {
    match app.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

fn request_timeout_response() -> Response {
    axum::http::StatusCode::REQUEST_TIMEOUT.into_response()
}

fn axum_request(request: lambda_http::Request) -> Result<Request, LambdaHttpAdapterError> {
    let (parts, body) = request.into_parts();
    let body = match body {
        lambda_http::Body::Empty => Body::empty(),
        lambda_http::Body::Text(text) => Body::from(text),
        lambda_http::Body::Binary(bytes) => Body::from(bytes),
        _ => return Err(LambdaHttpAdapterError::UnsupportedBody),
    };

    Ok(Request::from_parts(parts, body))
}

#[derive(Debug, thiserror::Error)]
enum LambdaHttpAdapterError {
    #[error("unsupported Lambda HTTP request body")]
    UnsupportedBody,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Bytes, to_bytes},
        extract::OriginalUri,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use lambda_http::lambda_runtime::Context;
    use listing_source_core::ListingSourceId;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use test_api::{IntegrationTestService, OpenSearch, Postgres, get_postgres_client};
    use user_core::access_token::Scope;

    #[allow(dead_code)]
    mod api_support {
        extern crate self as aura_historia_api;

        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/api_support/mod.rs"
        ));
    }

    const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
    const OPENSEARCH: OpenSearch = OpenSearch();

    #[tokio::test]
    async fn should_preserve_empty_response_semantics_from_http_api_v2()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_health.json"
        ))?
        .with_lambda_context(context_with_remaining(Duration::from_secs(60)));
        let app = Router::new().route("/health", get(|| async { StatusCode::NO_CONTENT }));

        let response = handle_request(app, request).await?;
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;

        assert_eq!(StatusCode::NO_CONTENT, status);
        assert!(body.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_base64_body_query_and_cookies_from_http_api_v2()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_binary.json"
        ))?
        .with_lambda_context(context_with_remaining(Duration::from_secs(60)));
        let app = Router::new().route("/binary", post(echo_request));

        let response = handle_request(app, request).await?;
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        let body = String::from_utf8(body.to_vec())?;

        assert_eq!(
            "tag=one%20two&tag=three|session=first; theme=dark|edge-request|0,1,254,255",
            body
        );
        Ok(())
    }

    #[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
    async fn should_traverse_the_composed_public_router_from_an_http_api_v2_event() {
        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
            let app = api_support::aura_api_app().await;
            let request = listing_sources_request()?;

            let response = handle_request(app.clone(), request).await?;
            assert_eq!(StatusCode::OK, response.status());
            assert_eq!(
                Some("no-store"),
                response
                    .headers()
                    .get(axum::http::header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            assert_eq!(
                Some("lambda-route-test"),
                response
                    .headers()
                    .get(crate::transport::CORRELATION_ID_HEADER)
                    .and_then(|value| value.to_str().ok())
            );
            assert_eq!(
                Some("*"),
                response
                    .headers()
                    .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .and_then(|value| value.to_str().ok())
            );
            let request_id = response
                .headers()
                .get(crate::transport::REQUEST_ID_HEADER)
                .ok_or_else(|| std::io::Error::other("missing request ID"))?
                .to_str()?;
            assert!(uuid::Uuid::parse_str(request_id).is_ok());
            let response_body = to_bytes(response.into_body(), usize::MAX).await?;
            let response_data: serde_json::Value = serde_json::from_slice(&response_body)?;
            assert_eq!(response_data["size"], 1);
            assert!(response_data["items"].is_array());

            let mut invalid_auth_request = listing_sources_request()?;
            invalid_auth_request.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                axum::http::HeaderValue::from_static("Bearer invalid"),
            );
            let invalid_auth_response = handle_request(app, invalid_auth_request).await?;
            assert_eq!(StatusCode::UNAUTHORIZED, invalid_auth_response.status());
            assert_eq!(
                Some("no-store"),
                invalid_auth_response
                    .headers()
                    .get(axum::http::header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
            );
            Ok(())
        }
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[test_api::aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH])]
    async fn should_persist_an_authenticated_partner_write_through_the_http_api_v2_adapter() {
        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
            let app = api_support::aura_api_app().await;
            let pool = get_postgres_client().await;
            api_support::seed_current_fx_snapshot(&pool).await;
            let listing_source_id = api_support::seed_listing_source().await;
            let user_id = api_support::seed_user("USER").await;
            api_support::seed_partnership_membership(user_id, listing_source_id).await;
            api_support::seed_operator_partnership_listing_source_grant(listing_source_id).await;
            let token = String::from(
                api_support::seed_access_token_for(
                    user_id,
                    std::collections::HashSet::from([Scope::ProductListingsWrite]),
                )
                .await,
            );
            let listing_source_id = ListingSourceId::try_from(listing_source_id)?.to_string();
            let source_listing_id = "lambda-v2-partner-write";
            let body = serde_json::json!([{
                "sourceListingId": source_listing_id,
                "title": { "text": "Lambda adapter cabinet", "language": "en" },
                "description": { "text": "Persisted through HTTP API v2.", "language": "en" },
                "availability": "AVAILABLE",
                "url": "https://partner.example/lambda-v2-partner-write",
                "images": []
            }])
            .to_string();
            let request = http_api_v2_json_request(
                "POST",
                &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
                &token,
                &body,
            )?;

            let response = handle_request(app.clone(), request).await?;
            assert_eq!(StatusCode::OK, response.status());
            assert_eq!(serde_json::json!([]), serde_json::from_slice::<serde_json::Value>(&to_bytes(response.into_body(), usize::MAX).await?)?);
            let persisted: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
            )
            .bind(ListingSourceId::try_from(listing_source_id.as_str())?.as_uuid())
            .bind(source_listing_id)
            .fetch_one(&pool)
            .await?;
            assert_eq!(1, persisted);

            let denied = http_api_v2_json_request(
                "POST",
                &format!("/api/v1/listing-sources/{listing_source_id}/product-listings"),
                "invalid",
                &body,
            )?;
            let denied_response = handle_request(app, denied).await?;
            assert_eq!(StatusCode::UNAUTHORIZED, denied_response.status());
            let after_denied: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
            )
            .bind(ListingSourceId::try_from(listing_source_id.as_str())?.as_uuid())
            .bind(source_listing_id)
            .fetch_one(&pool)
            .await?;
            assert_eq!(1, after_denied);
            Ok(())
        }
        .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn should_return_timeout_before_running_pending_handler_when_lambda_budget_is_exhausted()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_health.json"
        ))?
        .with_lambda_context(context_with_remaining(Duration::ZERO));
        let app = Router::new().route(
            "/health",
            get(|| async {
                std::future::pending::<()>().await;
                StatusCode::NO_CONTENT
            }),
        );

        let response = handle_request(app, request).await?;

        assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
        Ok(())
    }

    #[tokio::test]
    async fn should_reserve_the_api_deadline_for_router_refresh_and_composition() {
        let exhausted = compose_router_with_budget(Some(Duration::ZERO), || async {
            panic!("exhausted invocation must not compose a router")
        })
        .await;
        assert!(matches!(exhausted, Ok(None)));

        let timed_out = compose_router_with_budget(Some(Duration::from_millis(1)), || async {
            std::future::pending::<Result<Router, String>>().await
        })
        .await;
        assert!(matches!(timed_out, Ok(None)));

        let provider_failure = compose_router_with_budget(Some(Duration::from_secs(1)), || async {
            Err::<Router, _>("PostgreSQL credential refresh unavailable".to_owned())
        })
        .await;
        assert!(matches!(
            provider_failure,
            Err(ref error) if error == "PostgreSQL credential refresh unavailable"
        ));
    }

    fn listing_sources_request() -> Result<lambda_http::Request, lambda_http::Error> {
        Ok(lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_listing_sources.json"
        ))?
        .with_lambda_context(context_with_remaining(Duration::from_secs(60))))
    }

    fn http_api_v2_json_request(
        method: &str,
        path: &str,
        access_token: &str,
        body: &str,
    ) -> Result<lambda_http::Request, lambda_http::Error> {
        let event = serde_json::json!({
            "version": "2.0",
            "routeKey": "$default",
            "rawPath": path,
            "rawQueryString": "",
            "headers": {
                "authorization": format!("Bearer {access_token}"),
                "content-type": "application/json",
                "host": "api.example.test",
                "origin": "https://client.example.test"
            },
            "requestContext": {
                "stage": "$default",
                "http": {
                    "method": method,
                    "path": path,
                    "protocol": "HTTP/1.1",
                    "sourceIp": "127.0.0.1",
                    "userAgent": "aura-historia-api-lambda-test"
                }
            },
            "body": body,
            "isBase64Encoded": false
        });
        Ok(lambda_http::request::from_str(&event.to_string())?
            .with_lambda_context(context_with_remaining(Duration::from_secs(60))))
    }

    async fn echo_request(
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> String {
        let query = uri.query().unwrap_or_default();
        let cookies = headers
            .get("cookie")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let correlation_id = headers
            .get("x-correlation-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        let bytes = body.iter().map(u8::to_string).collect::<Vec<_>>().join(",");

        format!("{query}|{cookies}|{correlation_id}|{bytes}")
    }

    fn context_with_remaining(remaining: Duration) -> Context {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default();
        let mut context = Context::default();
        context.deadline =
            now.saturating_add(remaining.as_millis().min(u128::from(u64::MAX)) as u64);
        context
    }
}
