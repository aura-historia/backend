use application::operation_context::{CredentialCapability, Principal};
use async_trait::async_trait;
use aura_historia_mcp::{
    AccessClass, AdmissionGate, AdmissionRejection, AdmissionRequest, AnonymousGate,
    OperationDefinition, OperationError, OperationPolicy, Registry, TransportConfig, router,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderValue, Request, StatusCode},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arguments {
    value: String,
}
#[derive(Serialize, JsonSchema)]
struct Output {
    value: String,
    principal: String,
    request_id: String,
    correlation_id: String,
}

fn policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
fn config() -> TransportConfig {
    TransportConfig {
        allowed_hosts: vec!["mcp.example.test".into()],
        ..Default::default()
    }
}
fn operation(calls: Arc<AtomicUsize>) -> OperationDefinition {
    OperationDefinition::new(
        "testOperation",
        "Internal test operation",
        policy(),
        move |input: Arguments, context| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(Output {
                    value: input.value,
                    principal: context.operation.principal.kind().into(),
                    request_id: context.operation.request_id.to_string(),
                    correlation_id: context.operation.correlation_id.to_string(),
                })
            }
        },
    )
    .unwrap()
}
fn app(calls: Arc<AtomicUsize>) -> Router {
    router(
        config(),
        Registry::new([operation(calls)]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap()
}
fn modern(method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name":"boundary-test", "version":"1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    json!({"jsonrpc":"2.0", "id":1, "method":method, "params":params})
}
fn request(body: &Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", "mcp.example.test")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some(version) = body
        .pointer("/params/_meta/io.modelcontextprotocol~1protocolVersion")
        .and_then(Value::as_str)
    {
        request
            .headers_mut()
            .insert("mcp-protocol-version", version.parse().unwrap());
        request.headers_mut().insert(
            "mcp-method",
            body["method"].as_str().unwrap().parse().unwrap(),
        );
        if let Some(name) = body.pointer("/params/name").and_then(Value::as_str) {
            request
                .headers_mut()
                .insert("mcp-name", name.parse().unwrap());
        }
    }
    request
}
async fn json_response(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), 3 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn should_discover_current_protocol_and_keep_business_catalogue_empty_by_default() {
    let app = router(config(), Registry::default(), Arc::new(AnonymousGate)).unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("server/discover", json!({}))))
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("application/json", response.headers()["content-type"]);
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert!(!response.headers().contains_key("mcp-session-id"));
    let result = json_response(response).await;
    assert_eq!(
        json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26"]),
        result["result"]["supportedVersions"]
    );
    let response = app
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    assert_eq!(json!([]), json_response(response).await["result"]["tools"]);
}

#[tokio::test]
async fn should_call_injected_operation_with_request_and_correlation_metadata() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut request = request(&modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"hello"}}),
    ));
    request
        .headers_mut()
        .insert("x-correlation-id", HeaderValue::from_static("upstream-123"));
    let response = app(calls.clone()).oneshot(request).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!("upstream-123", response.headers()["x-correlation-id"]);
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = json_response(response).await;
    assert_eq!(
        json!({"value":"hello", "principal":"anonymous", "request_id":id, "correlation_id":"upstream-123"}),
        body["result"]["structuredContent"]
    );
    assert_eq!(1, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_initialize_and_call_every_supported_legacy_revision_without_a_session() {
    for version in ["2025-11-25", "2025-06-18", "2025-03-26"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = app(calls.clone());
        let initialize = json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":{"protocolVersion":version, "capabilities":{}, "clientInfo":{"name":"legacy", "version":"1"}}});
        let response = app.clone().oneshot(request(&initialize)).await.unwrap();
        assert_eq!(StatusCode::OK, response.status(), "{version}");
        assert!(!response.headers().contains_key("mcp-session-id"));
        assert_eq!(
            version,
            json_response(response).await["result"]["protocolVersion"]
        );
        let initialized = json!({"jsonrpc":"2.0", "method":"notifications/initialized"});
        let response = app.clone().oneshot(request(&initialized)).await.unwrap();
        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert!(
            to_bytes(response.into_body(), 1024)
                .await
                .unwrap()
                .is_empty()
        );
        let mut call = request(
            &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"testOperation", "arguments":{"value":"legacy"}}}),
        );
        call.headers_mut()
            .insert("mcp-protocol-version", version.parse().unwrap());
        call.headers_mut().insert(
            "mcp-session-id",
            HeaderValue::from_static("stale-ignored-session"),
        );
        let response = app.oneshot(call).await.unwrap();
        assert_eq!(StatusCode::OK, response.status());
        assert!(!response.headers().contains_key("mcp-session-id"));
        assert_eq!(
            "legacy",
            json_response(response).await["result"]["structuredContent"]["value"]
        );
        assert_eq!(1, calls.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn should_reject_header_body_disagreement_before_admission_or_business_work() {
    let admitted = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(
        config(),
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(CountingGate(admitted.clone())),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    for (name, value) in [
        ("mcp-method", "tools/list"),
        ("mcp-name", "otherOperation"),
        ("mcp-protocol-version", "2025-11-25"),
    ] {
        let mut call = request(&body);
        call.headers_mut().insert(name, value.parse().unwrap());
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(-32020, json_response(response).await["error"]["code"]);
    }
    let mut legacy = request(
        &json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"testOperation", "arguments":{"value":"private"}}}),
    );
    legacy
        .headers_mut()
        .insert("mcp-name", HeaderValue::from_static("otherOperation"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.clone().oneshot(legacy).await.unwrap().status()
    );
    let mut duplicate = request(&body);
    duplicate
        .headers_mut()
        .append("mcp-name", HeaderValue::from_static("testOperation"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.oneshot(duplicate).await.unwrap().status()
    );
    assert_eq!(0, admitted.load(Ordering::SeqCst));
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

struct CountingGate(Arc<AtomicUsize>);
#[async_trait]
impl AdmissionGate for CountingGate {
    async fn admit(&self, _: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Principal::Anonymous)
    }
}

#[tokio::test]
async fn should_require_current_routing_headers_and_metadata_without_executing_tools() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    for header in ["mcp-method", "mcp-name", "mcp-protocol-version"] {
        let body = modern(
            "tools/call",
            json!({"name":"testOperation", "arguments":{"value":"private"}}),
        );
        let mut call = request(&body);
        call.headers_mut().remove(header);
        assert_eq!(
            StatusCode::BAD_REQUEST,
            app.clone().oneshot(call).await.unwrap().status()
        );
    }
    let mut body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    body["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    let response = app.oneshot(request(&body)).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_decode_base64_mcp_name_before_matching_body() {
    let mut call = request(&modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"ok"}}),
    ));
    call.headers_mut().insert(
        "mcp-name",
        HeaderValue::from_static("=?base64?dGVzdE9wZXJhdGlvbg==?="),
    );
    let response = app(Arc::new(AtomicUsize::new(0)))
        .oneshot(call)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(
        "ok",
        json_response(response).await["result"]["structuredContent"]["value"]
    );
}

#[tokio::test]
async fn should_return_distinct_protocol_errors_for_unknown_method_tool_arguments_and_version() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    for (body, status, code) in [
        (
            modern("unknown/method", json!({})),
            StatusCode::NOT_FOUND,
            -32601,
        ),
        (
            modern("tools/call", json!({"name":"unknown", "arguments":{}})),
            StatusCode::OK,
            -32602,
        ),
        (
            modern(
                "tools/call",
                json!({"name":"testOperation", "arguments":{"value":42}}),
            ),
            StatusCode::OK,
            -32602,
        ),
    ] {
        let response = app.clone().oneshot(request(&body)).await.unwrap();
        assert_eq!(status, response.status());
        assert_eq!(code, json_response(response).await["error"]["code"]);
    }
    let mut body = modern("tools/list", json!({}));
    body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = "2099-01-01".into();
    let response = app.oneshot(request(&body)).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    let error = json_response(response).await;
    assert_eq!(-32022, error["error"]["code"]);
    assert!(
        error["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!("2026-07-28"))
    );
}

#[tokio::test]
async fn should_reject_present_unapproved_origins_and_preserve_exact_ports() {
    let mut config = config();
    config.allowed_origins = vec!["https://app.example.test".into()];
    let app = router(config, Registry::default(), Arc::new(AnonymousGate)).unwrap();
    for (origin, expected) in [
        ("https://app.example.test", StatusCode::OK),
        ("https://app.example.test:443", StatusCode::OK),
        ("https://app.example.test:444", StatusCode::FORBIDDEN),
        ("null", StatusCode::FORBIDDEN),
        ("https://evil.test", StatusCode::FORBIDDEN),
        ("https://app.example.test/path", StatusCode::FORBIDDEN),
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert("origin", origin.parse().unwrap());
        assert_eq!(
            expected,
            app.clone().oneshot(call).await.unwrap().status(),
            "{origin}"
        );
    }
}

#[tokio::test]
async fn should_reject_bad_host_oversized_malformed_and_batched_json_before_business_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    let body = modern(
        "tools/call",
        json!({"name":"testOperation", "arguments":{"value":"private"}}),
    );
    let mut call = request(&body);
    call.headers_mut()
        .insert("host", HeaderValue::from_static("attacker.test"));
    assert_eq!(
        StatusCode::FORBIDDEN,
        app.clone().oneshot(call).await.unwrap().status()
    );
    for (raw, expected) in [
        ("x".repeat(256 * 1024 + 1), StatusCode::PAYLOAD_TOO_LARGE),
        ("{private malformed".into(), StatusCode::BAD_REQUEST),
        (json!([body]).to_string(), StatusCode::BAD_REQUEST),
    ] {
        let mut call = request(&json!({}));
        *call.body_mut() = Body::from(raw);
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(expected, response.status());
        assert!(
            !json_response(response)
                .await
                .to_string()
                .contains("private")
        );
    }
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_reject_http_content_negotiation_and_unavailable_stream_routes() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    for (header, value, expected) in [
        (
            "content-type",
            "application/jsonx",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ),
        ("accept", "application/json", StatusCode::NOT_ACCEPTABLE),
        (
            "accept",
            "application/json;q=0,text/event-stream",
            StatusCode::NOT_ACCEPTABLE,
        ),
    ] {
        let mut call = request(&modern("tools/list", json!({})));
        call.headers_mut().insert(header, value.parse().unwrap());
        assert_eq!(expected, app.clone().oneshot(call).await.unwrap().status());
    }
    for method in ["GET", "DELETE"] {
        let mut call = request(&json!({}));
        *call.method_mut() = method.parse().unwrap();
        let response = app.clone().oneshot(call).await.unwrap();
        assert_eq!(StatusCode::METHOD_NOT_ALLOWED, response.status());
        assert_eq!("POST", response.headers()["allow"]);
    }
}

#[tokio::test]
async fn should_fail_closed_for_supplied_credentials_and_leave_protected_tools_discoverable() {
    let mut protected = policy();
    protected.access = AccessClass::Protected;
    protected.required_scopes = vec![CredentialCapability::UsersRead];
    let op = OperationDefinition::new(
        "privateOperation",
        "Private",
        protected,
        |_: Arguments, _| async { Ok(json!({})) },
    )
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let tool = &json_response(response).await["result"]["tools"][0];
    assert_eq!("privateOperation", tool["name"]);
    assert_eq!(
        json!(["users:read"]),
        tool["_meta"]["aura.historia/requiredScopes"]
    );
    assert_eq!(
        json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}),
        tool["annotations"]
    );
    let response = app
        .clone()
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"privateOperation","arguments":{"value":"private"}}),
        )))
        .await
        .unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("authorization", HeaderValue::from_static("Bearer secret"));
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
}

struct DelegatedGate;
#[async_trait]
impl AdmissionGate for DelegatedGate {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        if !request.headers.contains_key("authorization") {
            let mut error = AdmissionRejection::new(StatusCode::UNAUTHORIZED);
            error.headers.insert("www-authenticate",HeaderValue::from_static("Bearer resource_metadata=\"https://mcp.example.test/.well-known/oauth-protected-resource\""));
            return Err(error);
        }
        Ok(Principal::DelegatedUser {
            user_id: "usr_01h455vb4pex5vy7enb1p677vn".parse().unwrap(),
            capabilities: BTreeSet::from([CredentialCapability::UsersRead]),
        })
    }
}

#[tokio::test]
async fn should_challenge_before_sdk_dispatch_and_preserve_exact_delegated_capabilities() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let op = OperationDefinition::new("testOperation","Test",policy(), move |_: Arguments, context| {
        observed.fetch_add(1,Ordering::SeqCst);
        async move {
            assert!(matches!(context.operation.principal,Principal::DelegatedUser { capabilities, .. } if capabilities == BTreeSet::from([CredentialCapability::UsersRead])));
            Ok(json!({"allowed":true}))
        }
    }).unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(DelegatedGate),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation","arguments":{"value":"private"}}),
    );
    let response = app.clone().oneshot(request(&body)).await.unwrap();
    assert_eq!(StatusCode::UNAUTHORIZED, response.status());
    assert!(response.headers().contains_key("www-authenticate"));
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(0, calls.load(Ordering::SeqCst));
    let mut call = request(&body);
    call.headers_mut().insert(
        "authorization",
        HeaderValue::from_static("Bearer test-only"),
    );
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert_eq!(1, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_cancel_operation_at_deadline_and_when_request_is_dropped() {
    for disconnect in [false, true] {
        let started = Arc::new(tokio::sync::Notify::new());
        let token = Arc::new(tokio::sync::Mutex::new(None));
        let observed_token = token.clone();
        let observed_started = started.clone();
        let op = OperationDefinition::new(
            "testOperation",
            "Test",
            policy(),
            move |_: Arguments, context| {
                let token = observed_token.clone();
                let started = observed_started.clone();
                async move {
                    *token.lock().await = Some(context.cancellation);
                    started.notify_one();
                    std::future::pending::<Result<Value, OperationError>>().await
                }
            },
        )
        .unwrap();
        let mut config = config();
        config.request_timeout = Duration::from_millis(40);
        let app = router(
            config,
            Registry::new([op]).unwrap(),
            Arc::new(AnonymousGate),
        )
        .unwrap();
        let task = tokio::spawn(app.oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"pending"}}),
        ))));
        started.notified().await;
        if disconnect {
            task.abort();
            let _ = task.await;
        } else {
            assert_eq!(
                StatusCode::REQUEST_TIMEOUT,
                task.await.unwrap().unwrap().status()
            );
        }
        let cancellation: CancellationToken = token.lock().await.clone().unwrap();
        tokio::time::timeout(Duration::from_secs(1), cancellation.cancelled())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn should_bound_concurrent_request_admission_before_business_work() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let notify = entered.clone();
    let op = OperationDefinition::new("testOperation", "Test", policy(), move |_: Arguments, _| {
        let notify = notify.clone();
        async move {
            notify.notify_one();
            std::future::pending::<Result<Value, OperationError>>().await
        }
    })
    .unwrap();
    let mut config = config();
    config.max_concurrent_requests = 1;
    let app = router(
        config,
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let body = modern(
        "tools/call",
        json!({"name":"testOperation","arguments":{"value":"pending"}}),
    );
    let task = tokio::spawn(app.clone().oneshot(request(&body)));
    entered.notified().await;
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        app.oneshot(request(&body)).await.unwrap().status()
    );
    task.abort();
}

#[tokio::test]
async fn should_return_safe_distinct_operation_errors_without_private_failure_causes() {
    for (error, code, tool_code) in [
        (OperationError::InvalidInput, None, Some("INVALID_INPUT")),
        (OperationError::NotFound, None, Some("NOT_FOUND")),
        (OperationError::Conflict, None, Some("CONFLICT")),
        (
            OperationError::Unavailable(std::io::Error::other("private token secret").into()),
            None,
            Some("TEMPORARILY_UNAVAILABLE"),
        ),
        (
            OperationError::Internal(std::io::Error::other("private token secret").into()),
            Some(-32603),
            None,
        ),
    ] {
        let error = Arc::new(std::sync::Mutex::new(Some(error)));
        let op =
            OperationDefinition::new("testOperation", "Test", policy(), move |_: Arguments, _| {
                let error = error.lock().unwrap().take().unwrap();
                async { Err::<Value, _>(error) }
            })
            .unwrap();
        let app = router(
            config(),
            Registry::new([op]).unwrap(),
            Arc::new(AnonymousGate),
        )
        .unwrap();
        let body = json_response(
            app.oneshot(request(&modern(
                "tools/call",
                json!({"name":"testOperation","arguments":{"value":"private"}}),
            )))
            .await
            .unwrap(),
        )
        .await;
        assert!(!body.to_string().contains("private"));
        assert!(!body.to_string().contains("secret"));
        if let Some(code) = code {
            assert_eq!(code, body["error"]["code"]);
        }
        if let Some(code) = tool_code {
            assert_eq!(true, body["result"]["isError"]);
            assert_eq!(code, body["result"]["content"][0]["text"]);
        }
    }
}

#[tokio::test]
async fn should_reject_oversized_result_without_silently_truncating_it() {
    let op = OperationDefinition::new("testOperation", "Test", policy(), |_: Arguments, _| async {
        Ok("x".repeat(3 * 1024 * 1024))
    })
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"small"}}),
        )))
        .await
        .unwrap();
    let body = json_response(response).await;
    assert_eq!(-32603, body["error"]["code"]);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transport limit")
    );
    assert!(body.get("result").is_none());
}

#[tokio::test]
async fn should_work_with_official_sdk_clients_over_native_http_in_both_lifecycle_modes() {
    use rmcp::{
        ClientLifecycleMode, ClientServiceExt,
        model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = config();
    config.allowed_hosts = vec![address.to_string()];
    let calls = Arc::new(AtomicUsize::new(0));
    let app = router(
        config,
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let stop = CancellationToken::new();
    let shutdown = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    for lifecycle in [
        ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
        ClientLifecycleMode::Initialize,
    ] {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{address}/v1")),
        );
        let mut config = ClientConfig::default();
        config.protocol_version = ProtocolVersion::V_2025_11_25;
        let client = tokio::time::timeout(
            Duration::from_secs(5),
            config.serve_with_lifecycle(transport, lifecycle),
        )
        .await
        .unwrap()
        .unwrap();
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!("testOperation", tools.tools[0].name);
        let result = client
            .call_tool(
                CallToolRequestParams::new("testOperation")
                    .with_arguments(serde_json::from_value(json!({"value":"sdk-client"})).unwrap()),
            )
            .await
            .unwrap();
        assert_eq!(
            "sdk-client",
            serde_json::to_value(result).unwrap()["structuredContent"]["value"]
        );
        client.cancel().await.unwrap();
    }
    assert_eq!(2, calls.load(Ordering::SeqCst));
    stop.cancel();
    server.await.unwrap();
}

#[tokio::test]
async fn should_page_tools_in_stable_operation_id_order_and_reject_invalid_cursors() {
    let operations = (0..53).rev().map(|index| {
        let name: &'static str = Box::leak(format!("testOperation{index:02}").into_boxed_str());
        OperationDefinition::new(name, "Test", policy(), |_: Arguments, _| async {
            Ok(Output {
                value: "ok".into(),
                principal: "anonymous".into(),
                request_id: "request".into(),
                correlation_id: "request".into(),
            })
        })
        .unwrap()
    });
    let app = router(
        config(),
        Registry::new(operations).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let first = json_response(response).await;
    assert_eq!(50, first["result"]["tools"].as_array().unwrap().len());
    assert_eq!("testOperation00", first["result"]["tools"][0]["name"]);
    assert_eq!("testOperation49", first["result"]["nextCursor"]);
    let response = app
        .clone()
        .oneshot(request(&modern(
            "tools/list",
            json!({"cursor":first["result"]["nextCursor"]}),
        )))
        .await
        .unwrap();
    let second = json_response(response).await;
    assert_eq!(3, second["result"]["tools"].as_array().unwrap().len());
    assert_eq!("testOperation50", second["result"]["tools"][0]["name"]);
    assert!(second["result"].get("nextCursor").is_none());
    let response = app
        .oneshot(request(&modern("tools/list", json!({"cursor":"unknown"}))))
        .await
        .unwrap();
    assert_eq!(-32602, json_response(response).await["error"]["code"]);
}

#[tokio::test]
async fn should_preserve_legacy_json_schema_object_requirement_only_where_needed() {
    let ops = [
        OperationDefinition::new(
            "arrayOperation",
            "Array",
            policy(),
            |_: Arguments, _| async { Ok(vec!["one", "two"]) },
        )
        .unwrap(),
        OperationDefinition::new(
            "nullableOperation",
            "Nullable",
            policy(),
            |_: Arguments, _| async { Ok(None::<String>) },
        )
        .unwrap(),
    ];
    let app = router(
        config(),
        Registry::new(ops).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .clone()
        .oneshot(request(&modern("tools/list", json!({}))))
        .await
        .unwrap();
    let tools = json_response(response).await;
    for tool in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!("object", tool["outputSchema"]["type"]);
        assert!(tool["outputSchema"]["properties"].get("result").is_some());
    }
    for (name, expected) in [
        ("arrayOperation", json!({"result":["one","two"]})),
        ("nullableOperation", json!({"result":null})),
    ] {
        let response = app
            .clone()
            .oneshot(request(&modern(
                "tools/call",
                json!({"name":name,"arguments":{"value":"test"}}),
            )))
            .await
            .unwrap();
        assert_eq!(
            expected,
            json_response(response).await["result"]["structuredContent"]
        );
    }
}

#[derive(JsonSchema)]
struct BrokenOutput;
impl Serialize for BrokenOutput {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("private serialization detail"))
    }
}

#[tokio::test]
async fn should_preserve_genuine_serialization_failure_as_safe_internal_error() {
    let op = OperationDefinition::new("testOperation", "Test", policy(), |_: Arguments, _| async {
        Ok(BrokenOutput)
    })
    .unwrap();
    let app = router(
        config(),
        Registry::new([op]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"test"}}),
        )))
        .await
        .unwrap();
    let body = json_response(response).await;
    assert_eq!(-32603, body["error"]["code"]);
    assert_eq!("Operation failed", body["error"]["message"]);
}

#[tokio::test]
async fn should_reject_known_sdk_versions_outside_the_selected_compatibility_matrix() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    let mut call = request(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}));
    call.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2024-11-05"),
    );
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
    assert_eq!(-32022, json_response(response).await["error"]["code"]);
}

#[tokio::test]
async fn should_normalize_valid_multiple_accept_values_and_json_media_type_parameters() {
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("Application/JSON; charset=utf-8"),
    );
    call.headers_mut()
        .insert("accept", HeaderValue::from_static("application/json"));
    call.headers_mut()
        .append("accept", HeaderValue::from_static("text/event-stream"));
    let response = app(Arc::new(AtomicUsize::new(0)))
        .oneshot(call)
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
}

#[tokio::test]
async fn should_enforce_content_length_and_response_caps_on_admission_error_paths() {
    let mut config = config();
    config.max_response_bytes = 1024;
    let app = router(config, Registry::default(), Arc::new(AnonymousGate)).unwrap();
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("content-length", HeaderValue::from_static("262145"));
    assert_eq!(
        StatusCode::PAYLOAD_TOO_LARGE,
        app.clone().oneshot(call).await.unwrap().status()
    );
    let mut call = request(&modern("tools/list", json!({})));
    call.headers_mut()
        .insert("content-length", HeaderValue::from_static("invalid"));
    assert_eq!(
        StatusCode::BAD_REQUEST,
        app.clone().oneshot(call).await.unwrap().status()
    );
    let mut body = modern("tools/list", json!({}));
    body["id"] = "x".repeat(4096).into();
    let mut call = request(&body);
    call.headers_mut()
        .insert("mcp-method", HeaderValue::from_static("tools/call"));
    let response = app.oneshot(call).await.unwrap();
    assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
    assert!(bytes.len() < 1024);
    assert!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transport limit")
    );
}

struct PendingGate(Arc<tokio::sync::Mutex<Option<CancellationToken>>>);
#[async_trait]
impl AdmissionGate for PendingGate {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        *self.0.lock().await = Some(request.cancellation);
        std::future::pending().await
    }
}

#[tokio::test]
async fn should_apply_request_deadline_to_authentication_before_dispatch() {
    let cancellation = Arc::new(tokio::sync::Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut config = config();
    config.request_timeout = Duration::from_millis(10);
    let app = router(
        config,
        Registry::new([operation(calls.clone())]).unwrap(),
        Arc::new(PendingGate(cancellation.clone())),
    )
    .unwrap();
    let response = app
        .oneshot(request(&modern(
            "tools/call",
            json!({"name":"testOperation","arguments":{"value":"private"}}),
        )))
        .await
        .unwrap();
    assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
    assert_eq!(0, calls.load(Ordering::SeqCst));
    let token = cancellation.lock().await.clone().unwrap();
    assert!(token.is_cancelled());
}

#[test]
fn should_reject_invalid_registry_and_transport_configuration() {
    let calls = Arc::new(AtomicUsize::new(0));
    assert!(Registry::new([operation(calls.clone()), operation(calls)]).is_err());
    assert!(
        OperationDefinition::new("invalid name", "Test", policy(), |_: Arguments, _| async {
            Ok(())
        })
        .is_err()
    );
    assert!(
        OperationDefinition::new("testOperation", "Test", policy(), |_: String, _| async {
            Ok(())
        })
        .is_err()
    );
    let mut invalid_policy = policy();
    invalid_policy.destructive = true;
    assert!(
        OperationDefinition::new(
            "testOperation",
            "Test",
            invalid_policy,
            |_: Arguments, _| async { Ok(()) }
        )
        .is_err()
    );
    let mut invalid = config();
    invalid.allowed_origins = vec!["https://app.example.test:*".into()];
    assert!(router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err());
    let mut invalid = config();
    invalid.max_response_bytes = 2 * 1024 * 1024 + 1;
    assert!(router(invalid, Registry::default(), Arc::new(AnonymousGate)).is_err());
}
