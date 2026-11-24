use aura_historia_mcp::{McpDependencies, TransportConfig, mcp_router};
use aura_historia_runtime::{ProductListingConfig, ProductListingDependencies, Runtime};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{ClientConfig, ProtocolVersion},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};

pub type Client = RunningService<RoleClient, ClientConfig>;

struct NoEmbeddings;
#[async_trait::async_trait]
impl EmbeddingGenerator for NoEmbeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("consumer catalogue requested external embeddings")
    }
    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("consumer catalogue requested external embeddings")
    }
}

pub struct Server {
    pub http: reqwest::Client,
    pub origin: String,
    stop: CancellationToken,
}
impl Server {
    pub async fn start(pool: PgPool) -> Self {
        let runtime = Runtime::from_pool(pool);
        let config = ProductListingConfig::from_getter(|_| None).unwrap();
        let dependencies = McpDependencies {
            listings: ProductListingDependencies {
                opensearch: opensearch::OpenSearch::new(
                    opensearch::http::transport::Transport::single_node("http://127.0.0.1:9")
                        .unwrap(),
                ),
                embeddings: Arc::new(NoEmbeddings),
                fx_cache: config.fx_cache(),
                source_cache: config.source_cache(),
                read_execution_policy: config.read_execution_policy(),
            },
            search_intent_translator: Arc::new(
                aura_historia_runtime::UnavailableSearchIntentTranslator,
            ),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = mcp_router(
            TransportConfig {
                allowed_hosts: vec![address.to_string()],
                ..Default::default()
            },
            &runtime,
            dependencies,
        )
        .unwrap();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let stopping = stop.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel::<tokio::task::JoinHandle<()>>();
        test_api::TestContext::current()
            .unwrap()
            .on_shutdown(async move {
                stopping.cancel();
                receiver.await?.await?;
                Ok(())
            })
            .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
                .unwrap();
        });
        if let Err(task) = sender.send(task) {
            stop.cancel();
            task.await.unwrap();
            panic!("server shutdown ownership lost");
        }
        Self {
            http: reqwest::Client::new(),
            origin: format!("http://{address}"),
            stop,
        }
    }
    pub async fn client(&self, token: Option<&str>, lifecycle: ClientLifecycleMode) -> Client {
        let mut transport =
            StreamableHttpClientTransportConfig::with_uri(format!("{}/v1", self.origin));
        if let Some(token) = token {
            transport = transport.auth_header(token);
        }
        let mut config = ClientConfig::default();
        config.protocol_version = ProtocolVersion::V_2025_11_25;
        tokio::time::timeout(
            Duration::from_secs(5),
            config.serve_with_lifecycle(
                StreamableHttpClientTransport::from_config(transport),
                lifecycle,
            ),
        )
        .await
        .unwrap()
        .unwrap()
    }
    pub async fn rpc(
        &self,
        token: Option<&str>,
        method: &str,
        mut params: Value,
    ) -> reqwest::Response {
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"consumer-release","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
        let mut request = self
            .http
            .post(format!("{}/v1", self.origin))
            .header("accept", "application/json,text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", method)
            .header("mcp-session-id", "reused-untrusted-session");
        if let Some(name) = params["name"].as_str() {
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
        token: Option<&str>,
        name: &str,
        arguments: Value,
    ) -> reqwest::Response {
        self.rpc(
            token,
            "tools/call",
            json!({"name":name,"arguments":arguments}),
        )
        .await
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
pub fn discover() -> ClientLifecycleMode {
    ClientLifecycleMode::Discover {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
    }
}

/// Tools from both surviving manifests, keyed by name, with manifest-level policy defaults applied.
pub fn manifest_tools() -> BTreeMap<String, Value> {
    let mut expected = BTreeMap::new();
    for manifest in [
        include_str!("../../public-operations.json"),
        include_str!("../../authenticated-operations.json"),
    ] {
        let manifest: Value = serde_json::from_str(manifest).unwrap();
        for entry in manifest["operations"].as_array().unwrap() {
            let mut entry = entry.clone();
            for key in ["access", "requiredScopes", "annotations"] {
                if entry.get(key).is_none() {
                    entry[key] = manifest[key].clone();
                }
            }
            let name = entry["name"].as_str().unwrap().to_owned();
            assert_eq!(entry["name"], entry["operationId"]);
            assert!(expected.insert(name, entry).is_none());
        }
    }
    expected
}

/// The complete catalogue fits one page: no continuation is ever returned.
pub async fn catalogue(client: &Client) -> BTreeMap<String, Value> {
    let page = client.list_tools(None).await.unwrap();
    assert_eq!(37, page.tools.len());
    assert!(page.next_cursor.is_none());
    let mut tools = BTreeMap::new();
    for tool in page.tools {
        let value = serde_json::to_value(&tool).unwrap();
        assert!(tools.insert(tool.name.to_string(), value).is_none());
    }
    tools
}

pub struct Actor {
    pub user: UserId,
    pub token: String,
}
pub async fn actor(pool: &PgPool, scopes: &[&str]) -> Actor {
    let user = UserId::new();
    let token_id = AccessTokenId::new();
    let raw = RawAccessToken::new();
    let hash: HashedRawAccessToken = raw.clone().into();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,$2,'FREE','USER')")
        .bind(user.as_uuid())
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) VALUES($1,$2,$3,$4,'Consumer release fixture',$5,'USER')").bind(token_id.as_uuid()).bind(user.as_uuid()).bind(hash.short_token()).bind(hash.long_token_hash()).bind(scopes).execute(pool).await.unwrap();
    Actor {
        user,
        token: raw.into(),
    }
}
