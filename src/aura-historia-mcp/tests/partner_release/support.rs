use aura_historia_mcp::{McpDependencies, TransportConfig, mcp_router};
use aura_historia_runtime::{
    ProductListingConfig, ProductListingDependencies, ProductListingIngestionDependencies, Runtime,
};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use product_listing_ingestion_sqs::codec;
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProductListingIngestionCompletion, ProductListingIngestionItemOutcome,
        ProductListingIngestionMessage, ProductListingIngestionNotAttemptedReason as NotAttempted,
        ProductListingIngestionOutcome as Outcome,
        ProductListingIngestionRejectionReason as Rejection,
    },
};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use user_core::{
    access_token::{AccessTokenId, HashedRawAccessToken, RawAccessToken},
    user_id::UserId,
};

pub type Client = RunningService<RoleClient, ClientConfig>;
pub const SCOPES: &[&str] = &[
    "users:read",
    "listing-sources:read",
    "listing-sources:write",
    "product-listings:read",
    "product-listings:write",
    "parties:read",
    "parties:write",
];

#[derive(Clone, Default)]
pub struct Publisher {
    pub submissions: Arc<Mutex<Vec<Vec<ProductListingIngestionMessage>>>>,
    pub fail_before_sending: Arc<Mutex<bool>>,
    attempts: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl ProductListingIngestionPublisher for Publisher {
    async fn publish(
        &self,
        messages: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if *self.fail_before_sending.lock().unwrap() {
            return Err(ProductListingIngestionPublishError::new(
                std::io::Error::other("PRIVATE_PUBLISHER_CAUSE"),
            ));
        }
        let outcomes = messages
            .iter()
            .map(|message| {
                let outcome = match message.intent.source_listing_id().unwrap().as_ref() {
                    "unknown" => Outcome::Unconfirmed,
                    "rejected" => Outcome::Rejected {
                        reason: Rejection::Publisher {
                            code: "INVALID_MESSAGE_SIZE".into(),
                        },
                        retryable: false,
                    },
                    "not-attempted" => Outcome::NotAttempted {
                        reason: NotAttempted::DeadlineExceeded,
                        retryable: true,
                    },
                    _ => Outcome::Accepted,
                };
                ProductListingIngestionItemOutcome {
                    index: message.metadata.index,
                    command_id: message.metadata.command_id.clone(),
                    outcome,
                }
            })
            .collect();
        self.submissions.lock().unwrap().push(messages);
        Ok(outcomes)
    }
}
impl Publisher {
    pub fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    pub fn last(&self) -> Vec<ProductListingIngestionMessage> {
        self.submissions.lock().unwrap().last().unwrap().clone()
    }
    pub fn count(&self) -> usize {
        self.submissions.lock().unwrap().len()
    }
}
struct NoEmbeddings;
#[async_trait::async_trait]
impl EmbeddingGenerator for NoEmbeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("partner workflow requested external embeddings")
    }
    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("partner workflow requested external embeddings")
    }
}

pub struct Server {
    pub pool: PgPool,
    pub publisher: Publisher,
    pub http: reqwest::Client,
    pub origin: String,
    pub app: axum::Router,
    stop: CancellationToken,
}
impl Server {
    pub async fn start(pool: PgPool) -> Self {
        let runtime = Runtime::from_pool(pool.clone());
        let publisher = Publisher::default();
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
            ingestion: ProductListingIngestionDependencies::new(publisher.clone()),
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
        let router = app.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
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
            pool,
            publisher,
            http: reqwest::Client::new(),
            origin: format!("http://{address}"),
            app,
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
        params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"partner-release","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}});
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
    pub async fn apply(
        &self,
        message: ProductListingIngestionMessage,
    ) -> Result<
        ProductListingIngestionCompletion,
        product_listing_service::use_cases::ProductListingIngestionError,
    > {
        let envelope = codec::decode(&codec::encode(&message).unwrap())
            .unwrap()
            .into_service_envelope()
            .unwrap();
        product_listing_ingestion_lambda::compose_ingestion_use_case(self.pool.clone())
            .execute(envelope)
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

pub async fn catalogue(client: &Client) -> BTreeMap<String, Value> {
    let first = client.list_tools(None).await.unwrap();
    assert_eq!(50, first.tools.len());
    let second = client
        .list_tools(Some(
            rmcp::model::PaginatedRequestParams::default().with_cursor(first.next_cursor.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(24, second.tools.len());
    assert!(second.next_cursor.is_none());
    let mut tools = BTreeMap::new();
    for tool in first.tools.into_iter().chain(second.tools) {
        let value = serde_json::to_value(&tool).unwrap();
        assert!(tools.insert(tool.name.to_string(), value).is_none());
    }
    tools
}

pub async fn invoke(
    client: &Client,
    tools: &BTreeMap<String, Value>,
    name: &str,
    input: Value,
) -> Value {
    let tool = &tools[name];
    validate(&tool["inputSchema"], &input);
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(serde_json::from_value(input).unwrap()),
        )
        .await
        .unwrap();
    assert_ne!(Some(true), result.is_error, "{name}: {result:?}");
    let output = result.structured_content.unwrap();
    validate(&tool["outputSchema"], &output);
    let text: Value =
        serde_json::from_str(result.content[0].as_text().unwrap().text.as_str()).unwrap();
    assert_eq!(output, text);
    output
}
pub async fn tool_error(client: &Client, name: &str, input: Value, code: &str) {
    let result = client
        .call_tool(
            CallToolRequestParams::new(name.to_owned())
                .with_arguments(serde_json::from_value(input).unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(Some(true), result.is_error, "{name}: {result:?}");
    assert_eq!(code, result.content[0].as_text().unwrap().text);
}
pub fn validate(schema: &Value, value: &Value) {
    let validator = jsonschema::validator_for(schema).unwrap();
    assert!(
        validator.is_valid(value),
        "{}",
        validator
            .iter_errors(value)
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

pub struct Actor {
    pub user: UserId,
    pub token_id: AccessTokenId,
    pub token: String,
}
pub struct Source {
    pub party: PartyId,
    pub source: ListingSourceId,
    pub partnership: uuid::Uuid,
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
    sqlx::query("INSERT INTO access_tokens(access_token_id,user_id,token_short,token_hash,name,scopes,origin) VALUES($1,$2,$3,$4,'Partner release fixture',$5,'USER')").bind(token_id.as_uuid()).bind(user.as_uuid()).bind(hash.short_token()).bind(hash.long_token_hash()).bind(scopes).execute(pool).await.unwrap();
    Actor {
        user,
        token_id,
        token: raw.into(),
    }
}
pub async fn source(pool: &PgPool, actor: &Actor, name: &str) -> Source {
    let party = PartyId::new();
    let source = ListingSourceId::new();
    let partnership = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,$3)")
        .bind(party.as_uuid())
        .bind(format!("party-{}", party.as_uuid()))
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id,url) VALUES($1,$2,$3,$4,'https://shop.example.test')").bind(source.as_uuid()).bind(format!("source-{}",source.as_uuid())).bind(name).bind(party.as_uuid()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(party.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(actor.user.as_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.as_uuid()).execute(pool).await.unwrap();
    Source {
        party,
        source,
        partnership,
    }
}
pub fn product(id: &str) -> Value {
    json!({"sourceListingId":id,"title":{"language":"en","text":"Gallery painting"},"url":"https://shop.example.test/painting","images":[]})
}
pub fn batch(source: ListingSourceId, products: Vec<Value>) -> Value {
    json!({"listingSourceId":source,"products":products})
}
pub fn listing(source: ListingSourceId, id: &Value) -> Value {
    json!({"listingSourceId":source,"productListingId":id})
}
pub fn delivery(source: ListingSourceId, id: &str, version: u64) -> Value {
    json!({"listingSourceId":source,"sourceListingId":id,"expectedVersion":version,"delivery":{"state":"SET","value":{"integration":{"kind":"SOURCE"},"selection":{"state":"UNCHANGED"},"rateInputs":{"state":"UNCHANGED"},"shipping":{"state":"SET","value":{"kind":"REPLACE","value":{"kind":"PROHIBITED"}}},"collection":{"state":"UNCHANGED"}}}})
}
pub fn partner_names() -> BTreeSet<String> {
    serde_json::from_str::<Value>(include_str!("../../partner-operations.json")).unwrap()["operations"].as_array().unwrap().iter().map(|e|e["name"].as_str().unwrap().to_owned()).collect()
}
