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

struct CountingGate(Arc<AtomicUsize>);
#[async_trait]
impl AdmissionGate for CountingGate {
    async fn admit(&self, _: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Principal::Anonymous)
    }
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

#[derive(JsonSchema)]
struct BrokenOutput;
impl Serialize for BrokenOutput {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("private serialization detail"))
    }
}

struct PendingGate(Arc<tokio::sync::Mutex<Option<CancellationToken>>>);
#[async_trait]
impl AdmissionGate for PendingGate {
    async fn admit(&self, request: AdmissionRequest<'_>) -> Result<Principal, AdmissionRejection> {
        *self.0.lock().await = Some(request.cancellation);
        std::future::pending().await
    }
}

#[path = "transport/admission.rs"]
mod admission;
#[path = "transport/protocol.rs"]
mod protocol;
#[path = "transport/registry.rs"]
mod registry;
