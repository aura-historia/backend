use axum::{
    Router,
    body::Body,
    extract::Request,
    http::Uri,
    response::{IntoResponse, Response},
};
use lambda_http::{RequestExt, request::RequestContext};
use platform_lambda_bootstrap::{LambdaInvocationBudget, log_invocation_start};
use product_listing_ingestion_sqs::with_publication_deadline;
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
                let execute = async {
                    let app = match compose_router_with_budget(
                        budget.as_ref().map(LambdaInvocationBudget::remaining),
                        router_for_invocation,
                    )
                    .await?
                    {
                        Some(app) => app,
                        None => return Ok(request_timeout_response()),
                    };
                    handle_http_api_v2_request(app, request)
                        .await
                        .map_err(|error| error.to_string())
                };
                match budget.as_ref() {
                    Some(budget) => {
                        with_publication_deadline(publication_deadline(budget), execute).await
                    }
                    None => execute.await,
                }
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

/// Adapts one HTTP API v2 request to the fully composed Axum router.
///
/// This is the Lambda transport boundary. It carries no API Gateway-specific authorization or
/// application behavior; those remain normal HTTP concerns in the router.
pub async fn handle_http_api_v2_request(
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
        Some(budget) => {
            with_publication_deadline(
                publication_deadline(&budget),
                response_with_budget(app, request, budget.remaining()),
            )
            .await
        }
        None => response_from_app(app, request).await,
    })
}

fn publication_deadline(budget: &LambdaInvocationBudget) -> tokio::time::Instant {
    tokio::time::Instant::now()
        + budget
            .remaining()
            .saturating_sub(crate::transport::PUBLICATION_REPORT_HEADROOM)
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

fn axum_request(mut request: lambda_http::Request) -> Result<Request, LambdaHttpAdapterError> {
    // Named HTTP API stages can appear in the invocation path even when the matched
    // public route is unprefixed. Only remove the stage reported by API Gateway.
    if let Some(RequestContext::ApiGatewayV2(context)) = request.request_context_ref()
        && let Some(stage) = context.stage.as_deref()
        && let Some(path) = request.uri().path().strip_prefix(&format!("/{stage}/"))
    {
        let path = format!("/{path}");
        let path_and_query = match request.uri().query() {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        let mut uri = request.uri().clone().into_parts();
        uri.path_and_query = Some(
            path_and_query
                .parse()
                .map_err(|_| LambdaHttpAdapterError::InvalidPath)?,
        );
        *request.uri_mut() =
            Uri::from_parts(uri).map_err(|_| LambdaHttpAdapterError::InvalidPath)?;
    }

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
pub enum LambdaHttpAdapterError {
    #[error("unsupported Lambda HTTP request body")]
    UnsupportedBody,
    #[error("invalid HTTP API request path")]
    InvalidPath,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::get};
    use lambda_http::lambda_runtime::{Context, LambdaEvent};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn should_serialize_http_api_v2_no_content_cookies_and_binary_responses()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let no_content = serialize_http_api_v2_response(
            http_api_v2_event(include_str!("../tests/fixtures/http_api_v2_health.json"))?,
            SerializedHttpApiV2Response::NoContent,
        )
        .await?;
        assert_eq!(serde_json::json!(204), no_content["statusCode"]);
        assert_eq!(serde_json::json!(""), no_content["body"]);
        assert_eq!(serde_json::json!(false), no_content["isBase64Encoded"]);
        assert_eq!(serde_json::json!([]), no_content["cookies"]);

        let binary = serialize_http_api_v2_response(
            http_api_v2_event(include_str!("../tests/fixtures/http_api_v2_binary.json"))?,
            SerializedHttpApiV2Response::BinaryWithCookies,
        )
        .await?;
        assert_eq!(serde_json::json!(200), binary["statusCode"]);
        assert_eq!(serde_json::json!("AAH+/w=="), binary["body"]);
        assert_eq!(serde_json::json!(true), binary["isBase64Encoded"]);
        assert_eq!(
            serde_json::json!(["session=first; HttpOnly", "theme=dark; Secure"]),
            binary["cookies"]
        );
        assert_eq!(
            serde_json::json!("application/octet-stream"),
            binary["headers"]["content-type"]
        );
        assert!(binary["headers"].get("set-cookie").is_none());
        Ok(())
    }

    #[tokio::test]
    async fn should_route_both_named_stage_and_unprefixed_http_api_paths()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut event: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/http_api_v2_health.json"))?;
        event["routeKey"] = "GET /api/v1/health".into();
        event["requestContext"]["stage"] = "dev".into();
        event["requestContext"]["http"]["path"] = "/dev/api/v1/health".into();
        event["headers"]["host"] = "api.stage.aura-historia.com".into();
        event["rawQueryString"] = "probe=1".into();
        let app = Router::new().route(
            "/api/v1/health",
            get(|request: Request| async move {
                (
                    StatusCode::OK,
                    request.uri().query().unwrap_or_default().to_owned(),
                )
            }),
        );

        for path in ["/api/v1/health", "/dev/api/v1/health"] {
            event["rawPath"] = path.into();
            let request = lambda_http::request::from_str(&event.to_string())?;
            let response = handle_http_api_v2_request(app.clone(), request).await?;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), usize::MAX).await?,
                "probe=1"
            );
        }

        event["rawPath"] = "/other/api/v1/health".into();
        let request = lambda_http::request::from_str(&event.to_string())?;
        let response = handle_http_api_v2_request(app, request).await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_timeout_before_running_pending_handler_when_lambda_budget_is_exhausted()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_health.json"
        ))?
        .with_lambda_context(context_with_remaining(Duration::ZERO));
        let app = Router::new().route(
            "/api/v1/health",
            get(|| async {
                std::future::pending::<()>().await;
                StatusCode::NO_CONTENT
            }),
        );

        let response = handle_http_api_v2_request(app, request).await?;

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

    #[derive(Clone, Copy)]
    enum SerializedHttpApiV2Response {
        NoContent,
        BinaryWithCookies,
    }

    async fn serialize_http_api_v2_response(
        request: lambda_http::request::LambdaRequest,
        response: SerializedHttpApiV2Response,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        let service = lambda_http::service_fn(move |_request: lambda_http::Request| async move {
            serialized_http_api_v2_response(response)
        });
        let response = lambda_http::Adapter::from(service)
            .oneshot(LambdaEvent {
                payload: request,
                context: context_with_remaining(Duration::from_secs(60)),
            })
            .await?;
        Ok(serde_json::to_value(response)?)
    }

    fn serialized_http_api_v2_response(
        response: SerializedHttpApiV2Response,
    ) -> Result<lambda_http::http::Response<lambda_http::Body>, lambda_http::http::Error> {
        match response {
            SerializedHttpApiV2Response::NoContent => lambda_http::http::Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(lambda_http::Body::Empty),
            SerializedHttpApiV2Response::BinaryWithCookies => {
                lambda_http::http::Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/octet-stream")
                    .header("set-cookie", "session=first; HttpOnly")
                    .header("set-cookie", "theme=dark; Secure")
                    .body(lambda_http::Body::Binary(vec![0, 1, 254, 255]))
            }
        }
    }

    fn http_api_v2_event(
        event: &str,
    ) -> Result<lambda_http::request::LambdaRequest, serde_json::Error> {
        serde_json::from_str(event)
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
