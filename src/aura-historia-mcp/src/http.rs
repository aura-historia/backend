mod headers;
mod response;

use headers::*;
pub(crate) use response::deadline_response;
use response::{finalize_response, protocol_error};

use crate::{
    AdmissionGate, AdmissionRequest, ConfigError, InvocationContext, Registry, TransportConfig,
    config::normalized_origin, metadata::InvocationDeadline, protocol::ProtocolHandler,
};
use application::operation_context::{CorrelationId, OperationContext, RequestId};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

type SdkService = StreamableHttpService<ProtocolHandler, NeverSessionManager>;

#[derive(Clone)]
struct HttpState {
    config: TransportConfig,
    registry: Registry,
    gate: Arc<dyn AdmissionGate>,
    sdk: SdkService,
    admission: Arc<Semaphore>,
}

pub fn router(
    config: TransportConfig,
    registry: Registry,
    gate: Arc<dyn AdmissionGate>,
) -> Result<Router, ConfigError> {
    config.validate()?;
    let mut sdk_config = StreamableHttpServerConfig::default();
    sdk_config.legacy_session_mode = false;
    sdk_config.json_response = true;
    sdk_config.sse_keep_alive = None;
    sdk_config.sse_retry = None;
    sdk_config.max_request_body_bytes = config.max_request_bytes;
    sdk_config.allowed_hosts = config.allowed_hosts.clone();
    // Explicit default ports avoid the SDK's legacy port-wildcard interpretation.
    sdk_config.allowed_origins = config
        .allowed_origins
        .iter()
        .filter_map(|value| normalized_origin(value))
        .collect();
    sdk_config = sdk_config.enforce_origin_validation();
    let operations = registry.clone();
    let sdk = StreamableHttpService::new(
        move || {
            Ok(ProtocolHandler {
                registry: operations.clone(),
            })
        },
        Arc::new(NeverSessionManager::default()),
        sdk_config,
    );
    let state = HttpState {
        admission: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        config,
        registry,
        gate,
        sdk,
    };
    let response_limit = state.config.max_response_bytes;
    Ok(Router::new()
        .route("/v1", any(handle))
        .route("/health", get(probe))
        .route("/ready", get(probe))
        .with_state(state)
        .layer(axum::middleware::map_response(move |response| {
            finalize_response(response, response_limit)
        })))
}

async fn probe() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

async fn handle(State(state): State<HttpState>, request: Request) -> Response {
    let now = Instant::now();
    let invocation = request.extensions().get::<InvocationDeadline>();
    let deadline = invocation.map_or(now + state.config.request_timeout, |budget| {
        budget.deadline.min(now + state.config.request_timeout)
    });
    let request_id = invocation.map_or_else(
        || uuid::Uuid::new_v4().to_string(),
        |budget| budget.request_id.clone(),
    );
    let cancellation = CancellationToken::new();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let response = if deadline <= now {
        deadline_response()
    } else {
        match state.admission.clone().try_acquire_owned() {
            Err(_) => protocol_error(
                StatusCode::SERVICE_UNAVAILABLE,
                -32603,
                "Transport capacity exceeded",
                Value::Null,
            ),
            Ok(_permit) => match tokio::time::timeout_at(
                deadline,
                admitted_request(&state, request, &request_id, deadline, cancellation),
            )
            .await
            {
                Ok(response) if Instant::now() < deadline => response,
                Ok(_) | Err(_) => deadline_response(),
            },
        }
    };
    let mut response = response;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

async fn admitted_request(
    state: &HttpState,
    request: Request,
    request_id: &str,
    deadline: Instant,
    cancellation: CancellationToken,
) -> Response {
    if let Err(response) = validate_http_request(&request, &state.config) {
        return *response;
    }
    let (mut parts, body) = request.into_parts();
    let message = match read_message(body, state.config.max_request_bytes).await {
        Ok(message) => message,
        Err(response) => return *response,
    };
    let DecodedMessage {
        bytes,
        body,
        method,
        id,
    } = message;
    if let Err(response) = validate_message_headers(&parts.headers, &body, &method, &id) {
        return *response;
    }
    // The SDK owns protocol parsing, metadata requirements, version negotiation and
    // annotated parameter headers. The gate receives only the body identity after
    // every supplied routing identity agrees, including on legacy requests.
    let operation = (method == "tools/call")
        .then(|| body.pointer("/params/name").and_then(Value::as_str))
        .flatten()
        .and_then(|name| state.registry.get(name));
    let context = match admit(
        state,
        AdmissionRequest {
            headers: &parts.headers,
            method: &method,
            operation,
            deadline,
            cancellation,
        },
        request_id,
        &id,
    )
    .await
    {
        Ok(context) => context,
        Err(response) => return *response,
    };
    let correlation = context.operation.correlation_id.as_str().to_owned();
    parts.extensions.insert(context);
    if Instant::now() >= deadline {
        return deadline_response();
    }
    let request = Request::from_parts(parts, Body::from(bytes));
    invoke_sdk(state, request, id, &correlation).await
}

async fn invoke_sdk(
    state: &HttpState,
    mut request: Request,
    id: Value,
    correlation: &str,
) -> Response {
    // Normalize media headers after HTTP validation for the SDK's literal checks.
    request.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    request.headers_mut().insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    let response = match state.sdk.clone().oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (mut parts, body) = response.into_parts();
    // Business wrappers cannot emit SDK notifications or retain a peer; reject an
    // unexpected stream so neither front door ever needs a long-lived response.
    if parts
        .headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/event-stream"))
    {
        return protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            -32603,
            "Streaming is unavailable",
            id,
        );
    }
    let body = match to_bytes(Body::new(body), state.config.max_response_bytes).await {
        Ok(body) => body,
        Err(_) => {
            return protocol_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                -32603,
                "Response exceeds transport limit",
                Value::Null,
            );
        }
    };
    if let Ok(value) = HeaderValue::from_str(correlation) {
        parts.headers.insert("x-correlation-id", value);
    }
    Response::from_parts(parts, Body::from(body))
}

fn validate_http_request(request: &Request, config: &TransportConfig) -> Result<(), Box<Response>> {
    if let Err(status) = validate_origin_and_host(request.headers(), config) {
        return Err(Box::new(protocol_error(
            status,
            -32600,
            "HTTP admission rejected",
            Value::Null,
        )));
    }
    if request.method() != Method::POST {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return Err(Box::new(response));
    }
    if !valid_media_type(request.headers(), header::CONTENT_TYPE, "application/json") {
        return Err(Box::new(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response()));
    }
    if !valid_accept(request.headers()) {
        return Err(Box::new(StatusCode::NOT_ACCEPTABLE.into_response()));
    }
    match optional_header(request.headers(), "content-length") {
        Ok(Some(value)) => match value.parse::<usize>() {
            Ok(length) if length <= config.max_request_bytes => {}
            Ok(_) => {
                return Err(Box::new(protocol_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    -32600,
                    "Request body exceeds transport limit",
                    Value::Null,
                )));
            }
            Err(_) => {
                return Err(Box::new(protocol_error(
                    StatusCode::BAD_REQUEST,
                    -32600,
                    "Invalid content length",
                    Value::Null,
                )));
            }
        },
        Ok(None) => {}
        Err(_) => {
            return Err(Box::new(protocol_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid content length",
                Value::Null,
            )));
        }
    }
    Ok(())
}

struct DecodedMessage {
    bytes: Bytes,
    body: Value,
    method: String,
    id: Value,
}

async fn read_message(body: Body, limit: usize) -> Result<DecodedMessage, Box<Response>> {
    let bytes = match to_bytes(body, limit).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let exceeded_limit = error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
            return Err(Box::new(if exceeded_limit {
                protocol_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    -32600,
                    "Request body exceeds transport limit",
                    Value::Null,
                )
            } else {
                protocol_error(
                    StatusCode::BAD_REQUEST,
                    -32600,
                    "Unable to read request body",
                    Value::Null,
                )
            }));
        }
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return Err(Box::new(protocol_error(
                StatusCode::BAD_REQUEST,
                -32700,
                "Invalid JSON",
                Value::Null,
            )));
        }
    };
    let Some(method) = body
        .get("method")
        .and_then(Value::as_str)
        .filter(|_| body.get("jsonrpc") == Some(&json!("2.0")))
    else {
        return Err(Box::new(protocol_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Expected one JSON-RPC request or notification",
            Value::Null,
        )));
    };
    let method = method.to_owned();
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    Ok(DecodedMessage {
        bytes,
        body,
        method,
        id,
    })
}

async fn admit(
    state: &HttpState,
    request: AdmissionRequest<'_>,
    request_id: &str,
    id: &Value,
) -> Result<InvocationContext, Box<Response>> {
    let headers = request.headers;
    let deadline = request.deadline;
    let cancellation = request.cancellation.clone();
    let principal = match state.gate.admit(request).await {
        Ok(principal) => principal,
        Err(rejection) => {
            let mut response = rejection.status.into_response();
            response.headers_mut().extend(rejection.headers);
            return Err(Box::new(response));
        }
    };
    let correlation = match optional_header(headers, "x-correlation-id") {
        Ok(Some(value)) if valid_identifier(value) => value.to_owned(),
        Ok(None) => request_id.to_owned(),
        _ => {
            return Err(Box::new(protocol_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid correlation ID",
                id.clone(),
            )));
        }
    };
    Ok(InvocationContext {
        operation: OperationContext {
            principal,
            request_id: RequestId::new(request_id),
            correlation_id: CorrelationId::new(correlation),
        },
        deadline,
        cancellation,
        max_response_bytes: state.config.max_response_bytes,
    })
}

fn validate_message_headers(
    headers: &HeaderMap,
    body: &Value,
    method: &str,
    id: &Value,
) -> Result<(), Box<Response>> {
    if let Err(message) = validate_routing_headers(headers, body, method) {
        return Err(Box::new(protocol_error(
            StatusCode::BAD_REQUEST,
            -32020,
            message,
            id.clone(),
        )));
    }
    if method != "initialize"
        && let Some(version) = headers
            .get("mcp-protocol-version")
            .and_then(|value| value.to_str().ok())
        && !crate::SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .any(|supported| supported.as_str() == version)
    {
        return Err(Box::new((StatusCode::BAD_REQUEST, axum::Json(json!({
            "jsonrpc":"2.0", "id":id, "error": {"code":-32022,"message":"Unsupported protocol version", "data":{
                "requested":version,"supported":crate::SUPPORTED_PROTOCOL_VERSIONS.iter().map(|v|v.as_str()).collect::<Vec<_>>()
            }}
        }))).into_response()));
    }
    Ok(())
}
