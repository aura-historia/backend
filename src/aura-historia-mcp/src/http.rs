use crate::{
    AdmissionGate, AdmissionRequest, ConfigError, InvocationContext, Registry, TransportConfig,
    config::normalized_origin, metadata::InvocationDeadline, protocol::ProtocolHandler,
};
use application::operation_context::{CorrelationId, OperationContext, RequestId};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use base64::{Engine, engine::general_purpose::STANDARD};
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

async fn finalize_response(response: Response, limit: usize) -> Response {
    let (parts, body) = response.into_parts();
    let mut response = match to_bytes(body, limit).await {
        Ok(bytes) => Response::from_parts(parts, Body::from(bytes)),
        Err(_) => protocol_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            -32603,
            "Response exceeds transport limit",
            Value::Null,
        ),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
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
    if let Err(status) = validate_origin_and_host(request.headers(), &state.config) {
        return protocol_error(status, -32600, "HTTP admission rejected", Value::Null);
    }
    if request.method() != Method::POST {
        let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return response;
    }
    if !valid_media_type(request.headers(), header::CONTENT_TYPE, "application/json") {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    if !valid_accept(request.headers()) {
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }
    match optional_header(request.headers(), "content-length") {
        Ok(Some(value)) => match value.parse::<usize>() {
            Ok(length) if length <= state.config.max_request_bytes => {}
            Ok(_) => {
                return protocol_error(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    -32600,
                    "Request body exceeds transport limit",
                    Value::Null,
                );
            }
            Err(_) => {
                return protocol_error(
                    StatusCode::BAD_REQUEST,
                    -32600,
                    "Invalid content length",
                    Value::Null,
                );
            }
        },
        Ok(None) => {}
        Err(_) => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid content length",
                Value::Null,
            );
        }
    }
    let (mut parts, body) = request.into_parts();
    let bytes = match to_bytes(body, state.config.max_request_bytes).await {
        Ok(bytes) => bytes,
        Err(error) => {
            let exceeded_limit = error
                .source()
                .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
            return if exceeded_limit {
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
            };
        }
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return protocol_error(StatusCode::BAD_REQUEST, -32700, "Invalid JSON", Value::Null);
        }
    };
    let Some(method) = body
        .get("method")
        .and_then(Value::as_str)
        .filter(|_| body.get("jsonrpc") == Some(&json!("2.0")))
    else {
        return protocol_error(
            StatusCode::BAD_REQUEST,
            -32600,
            "Expected one JSON-RPC request or notification",
            Value::Null,
        );
    };
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    if let Err(message) = validate_routing_headers(&parts.headers, &body, method) {
        return protocol_error(StatusCode::BAD_REQUEST, -32020, message, id);
    }
    if method != "initialize"
        && let Some(version) = parts
            .headers
            .get("mcp-protocol-version")
            .and_then(|value| value.to_str().ok())
        && !crate::SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .any(|supported| supported.as_str() == version)
    {
        return (StatusCode::BAD_REQUEST, axum::Json(json!({
            "jsonrpc":"2.0", "id":id, "error": {"code":-32022,"message":"Unsupported protocol version", "data":{
                "requested":version,"supported":crate::SUPPORTED_PROTOCOL_VERSIONS.iter().map(|v|v.as_str()).collect::<Vec<_>>()
            }}
        }))).into_response();
    }
    // The SDK owns protocol parsing, metadata requirements, version negotiation and
    // annotated parameter headers. The gate receives only the body identity after
    // every supplied routing identity agrees, including on legacy requests.
    let operation = (method == "tools/call")
        .then(|| body.pointer("/params/name").and_then(Value::as_str))
        .flatten()
        .and_then(|name| state.registry.get(name));
    let principal = match state
        .gate
        .admit(AdmissionRequest {
            headers: &parts.headers,
            method,
            operation,
            deadline,
            cancellation: cancellation.clone(),
        })
        .await
    {
        Ok(principal) => principal,
        Err(rejection) => {
            let mut response = rejection.status.into_response();
            response.headers_mut().extend(rejection.headers);
            return response;
        }
    };
    let correlation = match optional_header(&parts.headers, "x-correlation-id") {
        Ok(Some(value)) if valid_identifier(value) => value.to_owned(),
        Ok(None) => request_id.to_owned(),
        _ => {
            return protocol_error(
                StatusCode::BAD_REQUEST,
                -32600,
                "Invalid correlation ID",
                id,
            );
        }
    };
    parts.extensions.insert(InvocationContext {
        operation: OperationContext {
            principal,
            request_id: RequestId::new(request_id),
            correlation_id: CorrelationId::new(correlation.clone()),
        },
        deadline,
        cancellation,
        max_response_bytes: state.config.max_response_bytes,
    });
    if Instant::now() >= deadline {
        return deadline_response();
    }
    // Normalize media headers after HTTP validation for the SDK's literal checks.
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    parts.headers.insert(
        header::ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    let request = Request::from_parts(parts, Body::from(bytes));
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
    if let Ok(value) = HeaderValue::from_str(&correlation) {
        parts.headers.insert("x-correlation-id", value);
    }
    Response::from_parts(parts, Body::from(body))
}

fn validate_routing_headers(
    headers: &HeaderMap,
    body: &Value,
    method: &str,
) -> Result<(), &'static str> {
    let version = optional_header(headers, "mcp-protocol-version")?;
    let body_version = if method == "initialize" {
        body.pointer("/params/protocolVersion")
    } else {
        body.pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
    }
    .and_then(Value::as_str);
    if method != "initialize" && body_version.is_some() && version.is_none() {
        return Err("Missing protocol version header");
    }
    if matches!((version, body_version), (Some(header), Some(body)) if header != body) {
        return Err("Protocol version header does not match body");
    }
    let modern = version
        .or(body_version)
        .is_some_and(|version| version >= "2026-07-28")
        && method != "initialize";
    match optional_header(headers, "mcp-method")? {
        Some(value) if value != method => return Err("Method header does not match body"),
        None if modern && body.get("id").is_some() => return Err("Missing method header"),
        _ => {}
    }
    let body_name = match method {
        "tools/call" | "prompts/get" => body.pointer("/params/name"),
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" => {
            body.pointer("/params/uri")
        }
        "tasks/get" | "tasks/update" | "tasks/cancel" => body.pointer("/params/taskId"),
        _ => None,
    }
    .and_then(Value::as_str);
    match optional_header(headers, "mcp-name")? {
        Some(value) if decode_name(value)?.as_deref() != body_name => {
            return Err("Name header does not match body");
        }
        None if modern && body_name.is_some() => return Err("Missing name header"),
        _ => {}
    }
    // Credentials and routing metadata must never have ambiguous multiple values.
    for name in ["authorization", "origin", "host", "mcp-session-id"] {
        optional_header(headers, name)?;
    }
    Ok(())
}

fn decode_name(value: &str) -> Result<Option<String>, &'static str> {
    let decoded = if let Some(encoded) = value
        .strip_prefix("=?base64?")
        .and_then(|s| s.strip_suffix("?="))
    {
        String::from_utf8(
            STANDARD
                .decode(encoded)
                .map_err(|_| "Malformed name header")?,
        )
        .map_err(|_| "Malformed name header")?
    } else {
        value.to_owned()
    };
    Ok(Some(decoded))
}

fn optional_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
) -> Result<Option<&'a str>, &'static str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err("Duplicate routing header");
    }
    value
        .map(|value| value.to_str().map_err(|_| "Malformed routing header"))
        .transpose()
}

fn validate_origin_and_host(
    headers: &HeaderMap,
    config: &TransportConfig,
) -> Result<(), StatusCode> {
    let host = optional_header(headers, "host")
        .map_err(|_| StatusCode::FORBIDDEN)?
        .ok_or(StatusCode::FORBIDDEN)?;
    if !config
        .allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if let Some(origin) = optional_header(headers, "origin").map_err(|_| StatusCode::FORBIDDEN)? {
        let origin = normalized_origin(origin).ok_or(StatusCode::FORBIDDEN)?;
        if !config
            .allowed_origins
            .iter()
            .any(|allowed| normalized_origin(allowed).as_ref() == Some(&origin))
        {
            return Err(StatusCode::FORBIDDEN);
        }
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn valid_media_type(headers: &HeaderMap, name: header::HeaderName, expected: &str) -> bool {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next().and_then(|v| v.to_str().ok()) else {
        return false;
    };
    values.next().is_none()
        && value
            .split(';')
            .next()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
}

fn valid_accept(headers: &HeaderMap) -> bool {
    let accepted = |expected: &str| {
        headers
            .get_all(header::ACCEPT)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|part| {
                let mut fields = part.trim().split(';');
                fields
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case(expected))
                    && !fields.any(|v| {
                        v.trim()
                            .strip_prefix("q=")
                            .and_then(|q| q.parse::<f32>().ok())
                            .is_some_and(|q| q <= 0.0)
                    })
            })
    };
    accepted("application/json") && accepted("text/event-stream")
}

pub(crate) fn deadline_response() -> Response {
    let mut response = protocol_error(
        StatusCode::REQUEST_TIMEOUT,
        -32603,
        "Request deadline exceeded",
        Value::Null,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn protocol_error(status: StatusCode, code: i32, message: &'static str, id: Value) -> Response {
    (
        status,
        axum::Json(
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        ),
    )
        .into_response()
}
