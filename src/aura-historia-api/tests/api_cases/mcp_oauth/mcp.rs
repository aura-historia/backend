use super::*;
use aura_historia_mcp::{TransportConfig, mcp_router};
use aura_historia_runtime::{ProductListingConfig, Runtime};

/// Starts the production registry and gate, with actual service handlers and the local DB.
pub struct McpServer {
    pub http: reqwest::Client,
    pub origin: String,
    pub pool: sqlx::PgPool,
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
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            http: reqwest::Client::new(),
            origin: format!("http://{address}"),
            pool,
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
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn successful_result(response: reqwest::Response) -> Value {
    assert_eq!(StatusCode::OK, response.status());
    let body: Value = response.json().await.unwrap();
    assert!(body.get("error").is_none(), "{body}");
    assert_ne!(json!(true), body["result"]["isError"], "{body}");
    body["result"]["structuredContent"].clone()
}
