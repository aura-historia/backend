//! `docs/mcp-tools.json` is the `tools` array of an anonymous current-revision `tools/list`.
//! The webapp registers its WebMCP tools from that file instead of listing them at runtime, so
//! this test renders the production catalogue through the router and fails when the file
//! differs. See `docs/mcp.md#tool-snapshot`.
use aura_historia_mcp::{McpDependencies, TransportConfig, mcp_router};
use aura_historia_runtime::{ProductListingConfig, ProductListingDependencies, Runtime};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use oauth_core::resource::AuraResourcePolicy;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tower::ServiceExt;

const PROTOCOL_VERSION: &str = "2026-07-28";
const HOST: &str = "mcp.snapshot.test";
/// Set to `1` to write the rendered snapshot instead of comparing it.
const UPDATE: &str = "UPDATE_MCP_TOOLS_SNAPSHOT";
const POLICIES: [AuraResourcePolicy; 2] = [
    AuraResourcePolicy::production(),
    AuraResourcePolicy::stage(),
];

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/mcp-tools.json")
}

/// Listing tools are only listed here; nothing may reach a provider.
struct NoEmbeddings;

#[async_trait::async_trait]
impl EmbeddingGenerator for NoEmbeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("tools/list requested an embedding")
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("tools/list requested an embedding")
    }
}

/// The production router of one environment, with a database and providers it never reaches.
fn production_router(policy: AuraResourcePolicy) -> Router {
    let unreachable = sqlx::postgres::PgConnectOptions::new_without_pgpass()
        .host("127.0.0.1")
        .port(9)
        .username("snapshot")
        .database("snapshot");
    let runtime =
        Runtime::from_pool(sqlx::postgres::PgPoolOptions::new().connect_lazy_with(unreachable));
    let config = ProductListingConfig::from_getter(|_| None).unwrap();
    let dependencies = McpDependencies {
        listings: ProductListingDependencies {
            opensearch: opensearch::OpenSearch::new(
                opensearch::http::transport::Transport::single_node("http://127.0.0.1:9").unwrap(),
            ),
            embeddings: Arc::new(NoEmbeddings),
            fx_cache: config.fx_cache(),
            source_cache: config.source_cache(),
            read_execution_policy: config.read_execution_policy(),
        },
    };
    mcp_router(
        TransportConfig {
            resource_policy: policy,
            allowed_hosts: vec![HOST.to_owned()],
            ..Default::default()
        },
        &runtime,
        dependencies,
    )
    .unwrap()
}

/// One anonymous `tools/list` page under the current revision, without `Authorization`.
async fn list_tools(router: &Router, cursor: Option<&str>) -> Value {
    let mut params = json!({
        "_meta": {
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": {"name": "tools-snapshot", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        }
    });
    if let Some(cursor) = cursor {
        params["cursor"] = json!(cursor);
    }
    let request = Request::builder()
        .method("POST")
        .uri("/v1")
        .header("host", HOST)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", PROTOCOL_VERSION)
        .header("mcp-method", "tools/list")
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": params})
                .to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(StatusCode::OK, response.status());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    body["result"].clone()
}

/// Every tool an anonymous client discovers, following `nextCursor` until it is absent.
async fn render(policy: AuraResourcePolicy) -> String {
    let router = production_router(policy);
    let mut tools = Vec::new();
    let mut cursor = None;
    loop {
        let page = list_tools(&router, cursor.as_deref()).await;
        tools.extend(page["tools"].as_array().unwrap().iter().cloned());
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    format!("{}\n", serde_json::to_string_pretty(&tools).unwrap())
}

#[tokio::test]
async fn should_commit_exactly_the_anonymous_tools_list_of_the_registry() {
    let rendered = render(AuraResourcePolicy::production()).await;
    let path = snapshot_path();
    if std::env::var_os(UPDATE).is_some_and(|value| value == "1") {
        std::fs::write(&path, &rendered).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == rendered,
        "docs/mcp-tools.json differs from the registry's anonymous tools/list. Regenerate it with \
         `{UPDATE}=1 cargo test --locked -p aura-historia-mcp --test tools_snapshot` and record \
         renamed or removed tools in docs/CHANGELOG.md."
    );
    let tools: Vec<Value> = serde_json::from_str(&committed).unwrap();
    assert!(
        tools
            .iter()
            .any(|tool| tool["_meta"]["aura.historia/access"] == "protected"),
        "protected tools stay discoverable anonymously"
    );
}

#[tokio::test]
async fn should_render_the_same_snapshot_for_every_environment_without_naming_one() {
    let mut rendered = Vec::new();
    for policy in POLICIES {
        rendered.push(render(policy).await);
    }
    assert!(rendered.windows(2).all(|pair| pair[0] == pair[1]));
    for policy in POLICIES {
        for origin in [
            policy.site_origin(),
            policy.rest_resource(),
            policy.mcp_origin(),
            policy.documentation_origin(),
        ] {
            assert!(
                !rendered[0].contains(origin),
                "a tool names the environment origin {origin}"
            );
        }
    }
}
