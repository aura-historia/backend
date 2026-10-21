use super::*;
use aura_historia_mcp::{TransportConfig, mcp_router};
use aura_historia_runtime::{ProductListingConfig, Runtime};

/// Starts the production registry and gate, with actual service handlers and the local DB.
pub struct McpServer {
    pub http: reqwest::Client,
    pub origin: String,
    pub pool: sqlx::PgPool,
    app: axum::Router,
    task: tokio::task::JoinHandle<()>,
}

impl McpServer {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let pool = get_postgres_client().await;
        let runtime = Runtime::from_pool(pool.clone());
        let app = mcp_router(
            TransportConfig {
                allowed_hosts: vec![address.to_string()],
                allowed_origins: vec!["https://generic-client.example".into()],
                ..Default::default()
            },
            &ProductListingConfig::from_getter(|_| None).unwrap(),
            &runtime,
        )
        .unwrap();
        let native = app.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, native).await.unwrap();
        });
        Self {
            http: reqwest::Client::new(),
            origin: format!("http://{address}"),
            pool,
            app,
            task,
        }
    }

    pub async fn request(
        &self,
        method: &str,
        mut params: Value,
        token: Option<&str>,
    ) -> reqwest::Response {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"generic-oauth-acceptance","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{}
        });
        let mut request = self
            .http
            .post(format!("{}/v1", self.origin))
            .header("accept", "application/json,text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", method)
            .header("mcp-session-id", "same-untrusted-session-for-every-user");
        if let Some(name) = params.get("name").and_then(Value::as_str) {
            request = request.header("mcp-name", name);
        }
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert_eq!("no-store", response.headers()["cache-control"]);
        assert!(response.headers().get("mcp-session-id").is_none());
        response
    }

    pub async fn call(
        &self,
        name: &str,
        arguments: Value,
        token: Option<&str>,
    ) -> reqwest::Response {
        self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            token,
        )
        .await
    }

    pub async fn tools(&self) -> Vec<Value> {
        let response = self.request("tools/list", json!({}), None).await;
        assert_eq!(StatusCode::OK, response.status());
        let body: Value = response.json().await.unwrap();
        assert!(body["result"].get("nextCursor").is_none());
        body["result"]["tools"].as_array().unwrap().clone()
    }

    pub async fn lambda_call(&self, name: &str, arguments: Value, token: Option<&str>) -> Value {
        use lambda_http::{
            lambda_runtime::{Context, LambdaEvent},
            request::LambdaRequest,
        };
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        use tower::ServiceExt;

        let mut headers = json!({
            "host":self.origin.strip_prefix("http://").unwrap(),
            "content-type":"application/json", "accept":"application/json,text/event-stream",
            "mcp-protocol-version":"2026-07-28", "mcp-method":"tools/call", "mcp-name":name
        });
        if let Some(token) = token {
            headers["authorization"] = json!(format!("Bearer {token}"));
        }
        let message = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{
            "name":name, "arguments":arguments, "_meta":{
                "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientInfo":{"name":"generic-oauth-lambda","version":"1"},
                "io.modelcontextprotocol/clientCapabilities":{}
            }
        }});
        let payload: LambdaRequest = serde_json::from_value(json!({
            "version":"2.0", "routeKey":"ANY /v1", "rawPath":"/v1", "rawQueryString":"",
            "headers":headers, "requestContext":{
                "stage":"$default", "http":{"method":"POST", "path":"/v1",
                "protocol":"HTTP/1.1", "sourceIp":"127.0.0.1", "userAgent":"generic-oauth-lambda"}
            }, "body":message.to_string(), "isBase64Encoded":false
        }))
        .unwrap();
        let app = self.app.clone();
        let service = lambda_http::service_fn(move |request| {
            aura_historia_mcp::lambda::handle_http_api_v2_request(app.clone(), request)
        });
        let mut context = Context::default();
        context.request_id = "generic-oauth-lambda".into();
        context.deadline = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
            + Duration::from_secs(30))
        .as_millis() as u64;
        let response = lambda_http::Adapter::from(service)
            .oneshot(LambdaEvent { payload, context })
            .await
            .unwrap();
        let response = serde_json::to_value(response).unwrap();
        assert_eq!("no-store", response["headers"]["cache-control"]);
        assert!(response["headers"].get("mcp-session-id").is_none());
        response
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn successful_result(response: reqwest::Response) -> Value {
    assert_eq!(StatusCode::OK, response.status());
    let body: Value = response.json().await.unwrap();
    successful_body(body)
}

pub fn successful_body(body: Value) -> Value {
    assert_eq!("2.0", body["jsonrpc"]);
    assert_eq!(1, body["id"]);
    assert!(body.get("error").is_none(), "{body}");
    let result = &body["result"];
    assert!(result.is_object());
    assert!(result.get("isError").is_none_or(|value| value == false));
    let structured = result["structuredContent"].clone();
    assert!(structured.is_object());
    let content = result["content"].as_array().unwrap();
    assert_eq!(1, content.len());
    assert_eq!("text", content[0]["type"]);
    let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(structured, text);
    structured
}

pub fn assert_shared_output<T: schemars::JsonSchema>(value: &Value) {
    let schema = schemars::generate::SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let schema = serde_json::to_value(schema).unwrap();
    assert!(jsonschema::validator_for(&schema).unwrap().is_valid(value));
}
