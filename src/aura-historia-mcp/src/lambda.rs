//! The production Lambda entry point and HTTP API v2 adaptation use the same router.
use crate::{REQUEST_TIMEOUT, http::deadline_response, metadata::InvocationDeadline};
use axum::{Router, body::Body, extract::Request, http::Uri, response::Response};
use lambda_http::{RequestExt, request::RequestContext};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use std::{future::Future, time::Duration};
use tower::ServiceExt;

/// Refreshes the composition before every invocation, within the same request deadline.
pub async fn run<F, Fut, E>(router_for_invocation: F) -> Result<(), lambda_http::Error>
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Router, E>> + Send,
    E: Into<lambda_http::Error> + Send,
{
    lambda_http::run(lambda_http::service_fn(move |request| {
        dispatch_invocation(request, router_for_invocation.clone())
    }))
    .await
}

async fn dispatch_invocation<F, Fut, E>(
    mut request: lambda_http::Request,
    compose: F,
) -> Result<Response, lambda_http::Error>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Router, E>>,
    E: Into<lambda_http::Error>,
{
    let invocation = invocation_deadline(&request);
    let app = match invocation {
        Some(invocation) => {
            if invocation.deadline <= tokio::time::Instant::now() {
                return Ok(deadline_response());
            }
            request.extensions_mut().insert(invocation.clone());
            match tokio::time::timeout_at(invocation.deadline, compose()).await {
                Ok(result) => result.map_err(Into::into)?,
                Err(_) => return Ok(deadline_response()),
            }
        }
        None => compose().await.map_err(Into::into)?,
    };
    handle_http_api_v2_request(app, request)
        .await
        .map_err(Into::into)
}

fn invocation_deadline(request: &lambda_http::Request) -> Option<InvocationDeadline> {
    request
        .extensions()
        .get::<InvocationDeadline>()
        .cloned()
        .or_else(|| {
            request.lambda_context_ref().map(|context| {
                let budget = LambdaInvocationBudget::from_context(
                    context,
                    REQUEST_TIMEOUT,
                    Duration::from_secs(1),
                );
                InvocationDeadline {
                    deadline: tokio::time::Instant::now() + budget.remaining(),
                    request_id: context.request_id.clone(),
                }
            })
        })
}

pub async fn handle_http_api_v2_request(
    app: Router,
    mut request: lambda_http::Request,
) -> Result<Response, LambdaAdapterError> {
    let invocation = invocation_deadline(&request);
    strip_reported_stage(&mut request)?;
    let (parts, body) = request.into_parts();
    let body = match body {
        lambda_http::Body::Empty => Body::empty(),
        lambda_http::Body::Text(body) => Body::from(body),
        lambda_http::Body::Binary(body) => Body::from(body),
        _ => return Err(LambdaAdapterError::Body),
    };
    let mut request = Request::from_parts(parts, body);
    if let Some(invocation) = invocation {
        if invocation.deadline <= tokio::time::Instant::now() {
            return Ok(deadline_response());
        }
        request.extensions_mut().insert(invocation);
    }
    Ok(match app.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    })
}

fn strip_reported_stage(request: &mut lambda_http::Request) -> Result<(), LambdaAdapterError> {
    if let Some(RequestContext::ApiGatewayV2(context)) = request.request_context_ref()
        && let Some(stage) = context.stage.as_deref()
        && stage != "$default"
        && let Some(path) = request.uri().path().strip_prefix(&format!("/{stage}/"))
    {
        let path = format!("/{path}");
        let path = match request.uri().query() {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        let mut uri = request.uri().clone().into_parts();
        uri.path_and_query = Some(
            path.parse()
                .map_err(|source| LambdaAdapterError::Path(Box::new(source)))?,
        );
        *request.uri_mut() =
            Uri::from_parts(uri).map_err(|source| LambdaAdapterError::Path(Box::new(source)))?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum LambdaAdapterError {
    #[error("invalid HTTP API v2 path")]
    Path(#[source] application::error::BoxError),
    #[error("unsupported Lambda HTTP body")]
    Body,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::get};

    fn request(deadline: tokio::time::Instant) -> lambda_http::Request {
        let mut request = lambda_http::Request::new(lambda_http::Body::Empty);
        *request.uri_mut() = "/ready".parse().unwrap();
        request.extensions_mut().insert(InvocationDeadline {
            deadline,
            request_id: "fixture-invocation".into(),
        });
        request
    }

    #[tokio::test(start_paused = true)]
    async fn should_skip_composition_after_the_invocation_deadline() {
        let composed = std::cell::Cell::new(false);
        let response = dispatch_invocation(request(tokio::time::Instant::now()), || {
            composed.set(true);
            std::future::ready(Ok::<_, std::io::Error>(Router::new()))
        })
        .await
        .unwrap();
        assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
        assert!(!composed.get());
    }

    #[tokio::test(start_paused = true)]
    async fn should_bound_pending_credential_refresh_by_the_invocation_deadline() {
        let start = tokio::time::Instant::now();
        let response = dispatch_invocation(request(start + Duration::from_secs(3)), || async {
            std::future::pending::<Result<Router, std::io::Error>>().await
        })
        .await
        .unwrap();
        assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
        assert_eq!(Duration::from_secs(3), start.elapsed());
    }

    #[tokio::test(start_paused = true)]
    async fn should_preserve_the_absolute_deadline_after_router_composition() {
        let start = tokio::time::Instant::now();
        let deadline = start + Duration::from_secs(25);
        let response = dispatch_invocation(request(deadline), || async {
            tokio::time::sleep(Duration::from_secs(20)).await;
            Ok::<_, std::io::Error>(Router::new().route(
                "/ready",
                get(move |request: Request| async move {
                    let invocation = request.extensions().get::<InvocationDeadline>().unwrap();
                    assert_eq!(deadline, invocation.deadline);
                    assert_eq!(
                        Duration::from_secs(5),
                        invocation.deadline - tokio::time::Instant::now()
                    );
                    StatusCode::NO_CONTENT
                }),
            ))
        })
        .await
        .unwrap();
        assert_eq!(StatusCode::NO_CONTENT, response.status());
    }

    #[tokio::test]
    async fn should_preserve_refresh_failures_instead_of_using_an_old_router() {
        let error = dispatch_invocation(
            request(tokio::time::Instant::now() + Duration::from_secs(5)),
            || async { Err::<Router, _>(std::io::Error::other("fixture refresh failure")) },
        )
        .await
        .unwrap_err();
        assert!(error.downcast_ref::<std::io::Error>().is_some());
    }
}
