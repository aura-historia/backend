use aura_historia_mcp::{
    AccessClass, AnonymousGate, MAX_RESPONSE_BYTES, OperationDefinition, OperationPolicy, Registry,
    TransportConfig, lambda::handle_http_api_v2_request, router,
};
use axum::{Router, body::to_bytes, http::StatusCode};
use lambda_http::{
    RequestExt,
    lambda_runtime::{Context, LambdaEvent},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;

#[derive(Deserialize, JsonSchema)]
struct Input {
    value: String,
}
#[derive(Serialize, JsonSchema)]
struct Output {
    value: String,
}

fn app(calls: Arc<AtomicUsize>) -> Router {
    let operation = OperationDefinition::new(
        "testOperation",
        "Internal test operation",
        OperationPolicy {
            access: AccessClass::Public,
            required_scopes: vec![],
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        move |input: Input, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(Output { value: input.value }) }
        },
    )
    .unwrap();
    router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new([operation]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap()
}
fn body(value: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"testOperation","arguments":{"value":value},"_meta":{
        "io.modelcontextprotocol/protocolVersion":"2026-07-28",
        "io.modelcontextprotocol/clientInfo":{"name":"lambda-test","version":"1"},
        "io.modelcontextprotocol/clientCapabilities":{}
    }}})
}
fn event(path: &str, stage: &str, value: &str) -> Value {
    json!({"version":"2.0","routeKey":"ANY /v1","rawPath":path,"rawQueryString":"", "headers":{
        "host":"mcp.example.test","content-type":"application/json","accept":"application/json, text/event-stream",
        "mcp-protocol-version":"2026-07-28","mcp-method":"tools/call","mcp-name":"testOperation"
    },"requestContext":{"stage":stage,"http":{"method":"POST","path":path,"protocol":"HTTP/1.1","sourceIp":"127.0.0.1","userAgent":"mcp-lambda-test"}},"body":body(value).to_string(),"isBase64Encoded":false})
}
fn context(remaining: Duration) -> Context {
    let mut context = Context::default();
    context.request_id = "lambda-invocation-123".into();
    context.deadline =
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() + remaining).as_millis() as u64;
    context
}

#[tokio::test]
async fn should_route_default_and_reported_named_stage_at_the_same_application_path() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone());
    for (path, stage, status) in [
        ("/v1", "$default", StatusCode::OK),
        ("/dev/v1", "dev", StatusCode::OK),
        ("/v1", "dev", StatusCode::OK),
        ("/other/v1", "dev", StatusCode::NOT_FOUND),
        ("/development/v1", "dev", StatusCode::NOT_FOUND),
    ] {
        let request = lambda_http::request::from_str(&event(path, stage, "ok").to_string())
            .unwrap()
            .with_lambda_context(context(Duration::from_secs(30)));
        let response = handle_http_api_v2_request(app.clone(), request)
            .await
            .unwrap();
        assert_eq!(status, response.status(), "{path}");
        assert_eq!("no-store", response.headers()["cache-control"]);
        if status == StatusCode::OK {
            assert_eq!("lambda-invocation-123", response.headers()["x-request-id"]);
            assert_eq!("application/json", response.headers()["content-type"]);
            let json: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), MAX_RESPONSE_BYTES)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!("ok", json["result"]["structuredContent"]["value"]);
        }
    }
    assert_eq!(3, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_reject_expired_invocation_before_running_business_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let request = lambda_http::request::from_str(&event("/v1", "$default", "private").to_string())
        .unwrap()
        .with_lambda_context(context(Duration::ZERO));
    let response = handle_http_api_v2_request(app(calls.clone()), request)
        .await
        .unwrap();
    assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert_eq!(0, calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_preserve_status_headers_json_and_no_session_through_actual_lambda_adapter() {
    let app = app(Arc::new(AtomicUsize::new(0)));
    let service =
        lambda_http::service_fn(move |request| handle_http_api_v2_request(app.clone(), request));
    let payload: lambda_http::request::LambdaRequest =
        serde_json::from_value(event("/v1", "$default", "escaped \" \\ \n value")).unwrap();
    let response = lambda_http::Adapter::from(service)
        .oneshot(LambdaEvent {
            payload,
            context: context(Duration::from_secs(30)),
        })
        .await
        .unwrap();
    let response = serde_json::to_value(response).unwrap();
    assert_eq!(200, response["statusCode"]);
    assert_eq!(false, response["isBase64Encoded"]);
    assert_eq!("application/json", response["headers"]["content-type"]);
    assert_eq!("no-store", response["headers"]["cache-control"]);
    assert!(response["headers"].get("mcp-session-id").is_none());
    let body: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        "escaped \" \\ \n value",
        body["result"]["structuredContent"]["value"]
    );
}

#[tokio::test]
async fn should_keep_adversarial_escaped_output_below_lambda_envelope_limit() {
    let operation = OperationDefinition::new(
        "testOperation",
        "Test",
        OperationPolicy {
            access: AccessClass::Public,
            required_scopes: vec![],
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        |_: Input, _| async {
            Ok(Output {
                value: "\"\\\n".repeat(90_000),
            })
        },
    )
    .unwrap();
    let app = router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new([operation]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let service =
        lambda_http::service_fn(move |request| handle_http_api_v2_request(app.clone(), request));
    let payload: lambda_http::request::LambdaRequest =
        serde_json::from_value(event("/v1", "$default", "small")).unwrap();
    let response = lambda_http::Adapter::from(service)
        .oneshot(LambdaEvent {
            payload,
            context: context(Duration::from_secs(30)),
        })
        .await
        .unwrap();
    let bytes = serde_json::to_vec(&response).unwrap();
    assert!(
        bytes.len() < 6 * 1024 * 1024,
        "actual envelope bytes: {}",
        bytes.len()
    );
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(200, response["statusCode"]);
    assert!(response["body"].as_str().unwrap().len() <= MAX_RESPONSE_BYTES);
    let body: Value = serde_json::from_str(response["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        "\"\\\n".repeat(90_000),
        body["result"]["structuredContent"]["value"]
    );
}

#[tokio::test]
async fn should_propagate_remaining_lambda_budget_to_active_operation_and_cancel_it() {
    let observed = Arc::new(tokio::sync::Mutex::new(None));
    let token = observed.clone();
    let operation = OperationDefinition::new(
        "testOperation",
        "Test",
        OperationPolicy {
            access: AccessClass::Public,
            required_scopes: vec![],
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        move |_: Input, context| {
            let token = token.clone();
            async move {
                *token.lock().await = Some(context.cancellation);
                std::future::pending::<Result<Output, aura_historia_mcp::OperationError>>().await
            }
        },
    )
    .unwrap();
    let app = router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        Registry::new([operation]).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    let request = lambda_http::request::from_str(&event("/v1", "$default", "small").to_string())
        .unwrap()
        .with_lambda_context(context(Duration::from_millis(1100)));
    let response = tokio::time::timeout(
        Duration::from_secs(1),
        handle_http_api_v2_request(app, request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(StatusCode::REQUEST_TIMEOUT, response.status());
    assert_eq!("no-store", response.headers()["cache-control"]);
    assert!(observed.lock().await.as_ref().unwrap().is_cancelled());
}
