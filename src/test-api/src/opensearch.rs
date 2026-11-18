use crate::IntegrationTestService;
use async_trait::async_trait;

use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::http::{Method, StatusCode, Url};
use opensearch::indices::{IndicesExistsParts, IndicesRefreshParts};
use opensearch::{DeleteByQueryParts, Error, GetParts, OpenSearch as Client};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use testcontainers::bollard::models::{HostConfig, PortBinding};
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt};
use tokio::sync::OnceCell;
use tokio::time::sleep;
use tracing::debug;

/// Name of the hybrid search pipeline — must stay in sync with
/// `product_listing_opensearch::product_listing_search_reader::HYBRID_SEARCH_PIPELINE_NAME`.
///
/// The search adapter's tests depend on `test-api`, so importing it here would be circular.
const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";

/// The production engine version. The immutable digest is the actual image policy, so mapping
/// compatibility is checked against the serving engine.
pub(crate) const OPENSEARCH_IMAGE: &str = "opensearchproject/opensearch:3.9.0@sha256:adfa61f85025d06b4aeb562e7e74fde7e31c437039c93c3862c17e9acebd6c7c";
pub(crate) const OPENSEARCH_CONTAINER_PORT: u16 = 9200;
/// Single-node settings shared by the supervisor's cluster and the process-owned cluster.
pub(crate) const OPENSEARCH_ENVIRONMENT: &[(&str, &str)] = &[
    ("discovery.type", "single-node"),
    ("DISABLE_SECURITY_PLUGIN", "true"),
    ("DISABLE_INSTALL_DEMO_CONFIG", "true"),
    ("OPENSEARCH_JAVA_OPTS", "-Xms512m -Xmx512m"),
    // A shared runner disk above the 90% high watermark makes this single-node test
    // cluster block index creation with 403 `index_create_block_exception`. Disk headroom
    // is guarded separately (`ci/guard-disk.py`), so the watermarks only cause flakes here.
    ("cluster.routing.allocation.disk.threshold_enabled", "false"),
];
const OPENSEARCH_CONTAINER_NAME_PREFIX: &str = "aura-historia-opensearch-test";
const OPENSEARCH_STARTUP_TIMEOUT: Duration = Duration::from_secs(180);

/// The process-owned cluster of an unmanaged run, started at most once per test process.
static PROCESS_CLUSTER: OnceCell<Url> = OnceCell::const_new();

/// Construct an owned client on the calling Tokio runtime.
///
/// HTTP pools contain runtime-owned dispatch tasks. Reusing them after that runtime stops,
/// including from a context cleanup thread or API server thread, can fail with DispatchGone.
/// Call this on each owning runtime; clones may share a transport within that runtime.
pub async fn get_opensearch_client() -> Client {
    let client = client_for(endpoint().await);
    debug!("Successfully initialized runtime-owned OpenSearch client.");
    client
}

fn client_for(endpoint: Url) -> Client {
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
        .disable_proxy()
        .build()
        .expect("shouldn't fail creating OpenSearch-Transport");
    Client::new(transport)
}

/// Marker type representing a real OpenSearch cluster in integration tests.
///
/// Implements the `IntegrationTestService` trait to support lifecycle management
/// when used with the `#[aura_integration_test]` macro.
///
pub struct OpenSearch();

#[async_trait]
impl IntegrationTestService for OpenSearch {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        let endpoint = endpoint().await;
        if let Some(context) = crate::TestContext::current() {
            let index = context
                .product_listings_index()
                .expect("scoped product index");
            context
                .persist_resources()
                .await
                .expect("persist scoped index");
            ensure_index_exists(
                &client_for(endpoint),
                &index,
                PRODUCT_LISTINGS_INDEX_MAPPING_STR,
            )
            .await
            .expect("create scoped product index");
        }
    }

    async fn tear_down(&self) {
        if crate::TestContext::current().is_some() {
            return;
        }
        clear_all_indices().await;
    }
}

/// The supervisor's cluster in a managed run; otherwise the process-owned cluster.
async fn endpoint() -> Url {
    if let Some(config) = crate::environment::managed_environment() {
        return Url::parse(
            config
                .opensearch_endpoint()
                .expect("managed profile has no OpenSearch"),
        )
        .expect("managed endpoint was validated");
    }
    PROCESS_CLUSTER
        .get_or_init(start_process_cluster)
        .await
        .clone()
}

/// Unique per process, so a process in another PID namespace on the same daemon never
/// removes this cluster.
fn opensearch_container_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        format!(
            "{OPENSEARCH_CONTAINER_NAME_PREFIX}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        )
    })
}

fn docker_remove(name: &str) {
    let _ = Command::new("docker")
        .args(["rm", "-f", "-v", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

extern "C" fn cleanup() {
    docker_remove(opensearch_container_name());
}

/// Removes the process-owned cluster on normal exit (`atexit`) and on `SIGINT` / `SIGTERM`.
fn install_cleanup() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        unsafe { libc::atexit(cleanup) };
        crate::signal::register_signal_cleanup(|| cleanup());
    });
}

/// Starts the pinned engine with only its HTTP port published on a random loopback port, then
/// installs the search pipeline and the unscoped product index. The container lives for the
/// whole test process.
async fn start_process_cluster() -> Url {
    install_cleanup();
    let name = opensearch_container_name();
    // A failed or cancelled start is retried by the OnceCell; never reuse its container.
    docker_remove(name);
    let started = Instant::now();
    let (repository, tag) = OPENSEARCH_IMAGE
        .split_once(':')
        .expect("OpenSearch image reference must be repository:tag");
    let request = GenericImage::new(repository, tag)
        .with_exposed_port(OPENSEARCH_CONTAINER_PORT.tcp())
        .with_container_name(name)
        .with_host_config_modifier(publish_http_on_loopback);
    let container = OPENSEARCH_ENVIRONMENT
        .iter()
        .fold(request, |request, (key, value)| {
            request.with_env_var(*key, *value)
        })
        .start()
        .await
        .expect("shouldn't fail starting OpenSearch test container");
    let port = container
        .get_host_port_ipv4(OPENSEARCH_CONTAINER_PORT.tcp())
        .await
        .expect("OpenSearch container must publish its HTTP port");
    let endpoint =
        Url::parse(&format!("http://localhost:{port}")).expect("OpenSearch endpoint must be a URL");
    let client = client_for(endpoint.clone());
    wait_until_reachable(&client).await;
    bootstrap_indices(&client)
        .await
        .expect("shouldn't fail setting up OpenSearch indices");
    debug!(
        elapsed_ms = started.elapsed().as_millis(),
        pid = std::process::id(),
        "OpenSearch container started and bootstrapped."
    );
    // The cleanup hooks remove the container when the process exits.
    std::mem::forget(container);
    endpoint
}

/// The security plugin is disabled, so the cluster must not be reachable from other hosts.
fn publish_http_on_loopback(host_config: &mut HostConfig) {
    host_config.publish_all_ports = Some(false);
    host_config.port_bindings = Some(HashMap::from([(
        OPENSEARCH_CONTAINER_PORT.tcp().to_string(),
        Some(vec![PortBinding {
            host_ip: Some("127.0.0.1".to_owned()),
            host_port: None,
        }]),
    )]));
}

async fn wait_until_reachable(client: &Client) {
    let deadline = Instant::now() + OPENSEARCH_STARTUP_TIMEOUT;
    loop {
        match client.ping().send().await {
            Ok(response) if response.status_code().is_success() => return,
            outcome if Instant::now() >= deadline => {
                panic!("OpenSearch test container did not become reachable: {outcome:?}")
            }
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
}

static PRODUCT_LISTINGS_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/product_listings.json"
));

fn check_status_allow_not_found(response: &Response) -> Result<(), Error> {
    if let Err(err) = response.error_for_status_code_ref()
        && err.status_code() != Some(StatusCode::NOT_FOUND)
    {
        return Err(err);
    }
    Ok(())
}

async fn ensure_index_exists(
    client: &Client,
    index: &str,
    mapping: &'static str,
) -> Result<(), Error> {
    let mapping: serde_json::Value =
        serde_json::from_str(mapping).expect("shouldn't fail parsing OpenSearch index mapping");
    let exists_response = client
        .indices()
        .exists(IndicesExistsParts::Index(&[index]))
        .send()
        .await?;
    check_status_allow_not_found(&exists_response)?;

    if exists_response.status_code().is_success() {
        debug!("OpenSearch index '{index}' already exists, skipping creation");
    } else {
        debug!("OpenSearch index '{index}' does not exist, creating it");
        client
            .indices()
            .create(opensearch::indices::IndicesCreateParts::Index(index))
            .body(mapping)
            .send()
            .await?
            .error_for_status_code()?;
    }

    client
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index]))
        .send()
        .await?
        .error_for_status_code()?;
    Ok(())
}

/// Registers the hybrid search pipeline on the local OpenSearch cluster.
///
/// Uses [`HYBRID_SEARCH_PIPELINE_NAME`] as the pipeline ID so it matches what
/// the product listing search reader references via the
/// `search_pipeline` query parameter.
///
/// Registers a `score-ranker-processor` pipeline using Reciprocal Rank Fusion (RRF) —
/// the same native hybrid-fusion technique used in production.
///
/// Returns the original registration failure so bootstrap can retry readiness or report
/// its cause before a test reaches a hybrid query.
async fn register_hybrid_search_pipeline(client: &Client) -> Result<(), Error> {
    // The pipeline name constant contains only alphanumeric characters and hyphens, which
    // are safe to embed in a URL path without encoding.
    debug_assert!(
        HYBRID_SEARCH_PIPELINE_NAME
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "HYBRID_SEARCH_PIPELINE_NAME must contain only alphanumeric characters and hyphens"
    );
    let path = format!("_search/pipeline/{HYBRID_SEARCH_PIPELINE_NAME}");

    let rrf_body = json!({
        "description": "Hybrid BM25+kNN search pipeline using Reciprocal Rank Fusion",
        "phase_results_processors": [
            {
                "score-ranker-processor": {
                    "combination": {
                        "technique": "rrf"
                    }
                }
            }
        ]
    });

    client
        .send(
            Method::Put,
            &path,
            HeaderMap::new(),
            None::<&serde_json::Value>,
            Some(JsonBody::new(rrf_body)),
            None,
        )
        .await?
        .error_for_status_code()?;
    Ok(())
}

async fn set_up_indices(client: &Client) -> Result<(), Error> {
    // Register the native-hybrid RRF search pipeline before creating indices.
    register_hybrid_search_pipeline(client).await?;

    ensure_index_exists(
        client,
        "product-listings",
        PRODUCT_LISTINGS_INDEX_MAPPING_STR,
    )
    .await?;

    Ok(())
}

pub(crate) async fn bootstrap_indices(client: &Client) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match set_up_indices(client).await {
            Ok(()) => return Ok(()),
            Err(err) if Instant::now() < deadline => {
                debug!(error = %err, "OpenSearch indices are not ready yet; retrying setup.");
                sleep(Duration::from_secs(2)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Clears all documents from every standard index to ensure test isolation.
///
/// Silently skips any index that does not yet exist (e.g. before the first
/// test has caused its creation).
pub(crate) async fn clear_all_indices() {
    let started = Instant::now();
    const INDICES: &[&str] = &["product-listings", "users"];
    for index in INDICES {
        match clear_index_data(index).await {
            Ok(_) => {}
            Err(e) => {
                debug!("Skipping clear for OpenSearch index '{index}' (may not exist yet): {e}")
            }
        }
    }
    debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "Cleared OpenSearch documents for test isolation."
    );
}

/// Clears all documents from the specified OpenSearch index.
///
/// This function uses the delete-by-query API to remove all documents while
/// preserving the index structure and mappings.
async fn clear_index_data(index: &str) -> Result<Response, Error> {
    let query = json!({
        "query": {
            "match_all": {}
        }
    });

    get_opensearch_client()
        .await
        .delete_by_query(DeleteByQueryParts::Index(&[index]))
        .body(query)
        .refresh(true)
        .send()
        .await?
        .error_for_status_code()
}

pub async fn read_by_id<T: DeserializeOwned>(index: &str, id: impl Into<String>) -> T {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    assert!(get_response.status_code().is_success());

    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    serde_json::from_value(response_doc["_source"].clone()).unwrap()
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::{Isolation, TestContext};
    use opensearch::IndexParts;
    use product_listing_core::product_listing_id::ProductListingId;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn should_use_fresh_clients_for_setup_reads_and_cleanup_after_prior_runtimes_stop() {
        let setup = runtime();
        let id = ProductListingId::new().to_string();
        let (session, index) = setup.block_on(async {
            let session = TestContext::acquire("OpenSearch runtime lifetime", Isolation::Isolated)
                .await
                .unwrap();
            let context = session.context().clone();
            let index = context
                .scope(async {
                    OpenSearch().set_up().await;
                    let index = context.product_listings_index().unwrap();
                    get_opensearch_client()
                        .await
                        .index(IndexParts::IndexId(&index, &id))
                        .body(json!({"productListingId": id}))
                        .send()
                        .await
                        .unwrap()
                        .error_for_status_code()
                        .unwrap();
                    refresh_index(&index).await;
                    index
                })
                .await;
            (session, index)
        });
        drop(setup);

        let reads = runtime();
        reads.block_on(session.context().scope(async {
            let source: serde_json::Value = read_by_id(&index, &id).await;
            assert_eq!(id, source["productListingId"]);
            assert!(
                get_opensearch_client()
                    .await
                    .ping()
                    .send()
                    .await
                    .unwrap()
                    .status_code()
                    .is_success()
            );
        }));
        drop(reads);

        let cleanup = runtime();
        cleanup.block_on(async {
            session.finish().await.unwrap();
            let response = get_opensearch_client()
                .await
                .indices()
                .exists(IndicesExistsParts::Index(&[&index]))
                .send()
                .await
                .unwrap();
            assert_eq!(StatusCode::NOT_FOUND, response.status_code());
        });
    }
}
