use axum::{Router, body::Body, extract::Request, response::Response};
use lambda_http::RequestExt;
use platform_lambda_bootstrap::log_invocation_start;
use tower::ServiceExt;

const COMPONENT: &str = "aura-historia-api";

pub async fn run(app: Router) -> Result<(), lambda_http::Error> {
    lambda_http::run(lambda_http::service_fn(move |request| {
        let app = app.clone();
        async move {
            handle_request(app, request)
                .await
                .map_err(|error| error.to_string())
        }
    }))
    .await
}

async fn handle_request(
    app: Router,
    request: lambda_http::Request,
) -> Result<Response, LambdaHttpAdapterError> {
    if let Some(context) = request.lambda_context_ref() {
        log_invocation_start(COMPONENT, context);
    }

    let response = match app.oneshot(axum_request(request)?).await {
        Ok(response) => response,
        Err(never) => match never {},
    };

    Ok(response)
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

    #[tokio::test]
    async fn should_preserve_empty_response_semantics_from_http_api_v2()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = lambda_http::request::from_str(include_str!(
            "../tests/fixtures/http_api_v2_health.json"
        ))?;
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
        ))?;
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
}
