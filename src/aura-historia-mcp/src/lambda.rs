//! The production Lambda entry point and HTTP API v2 adaptation use the same router.
use crate::{REQUEST_TIMEOUT, http::deadline_response, metadata::InvocationDeadline};
use axum::{Router, body::Body, extract::Request, http::Uri, response::Response};
use lambda_http::{RequestExt, request::RequestContext};
use platform_lambda_bootstrap::LambdaInvocationBudget;
use std::time::Duration;
use tower::ServiceExt;

pub async fn run(app: Router) -> Result<(), lambda_http::Error> {
    lambda_http::run(lambda_http::service_fn(move |request| {
        let app = app.clone();
        async move {
            handle_http_api_v2_request(app, request)
                .await
                .map_err(lambda_http::Error::from)
        }
    }))
    .await
}

pub async fn handle_http_api_v2_request(
    app: Router,
    mut request: lambda_http::Request,
) -> Result<Response, LambdaAdapterError> {
    let invocation = request.lambda_context_ref().map(|context| {
        let budget =
            LambdaInvocationBudget::from_context(context, REQUEST_TIMEOUT, Duration::from_secs(1));
        InvocationDeadline {
            deadline: tokio::time::Instant::now() + budget.remaining(),
            request_id: context.request_id.clone(),
        }
    });
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
