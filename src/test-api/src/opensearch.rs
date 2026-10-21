use crate::IntegrationTestService;
use crate::aws::{aws_emulator_container_name, get_aws_config, get_endpoint_url};
use application::error::{BoxError, box_error};
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
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::sleep;
use tracing::debug;

/// Name of the hybrid search pipeline — must stay in sync with
/// `product_listing_opensearch::product_listing_search_reader::HYBRID_SEARCH_PIPELINE_NAME`.
///
/// The search adapter's tests depend on `test-api`, so importing it here would be circular.
const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";

pub const TEST_DOMAIN_NAME: &str = "test-domain";

/// A lazily-initialized, globally shared OpenSearch client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static OPENSEARCH_CLIENT: OnceCell<Client> = OnceCell::const_new();
static OPENSEARCH_BOOTSTRAPPED: OnceCell<()> = OnceCell::const_new();

fn test_access_policies() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": "*",
                "Action": "es:*",
                "Resource": "*"
            }
        ]
    })
    .to_string()
}

/// Returns a shared `opensearch::OpenSearch`-Client for interacting with Floci.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, Floci endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_opensearch_client() -> &'static Client {
    let client = OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let endpoint_url = domain_endpoint().await;
            let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
                .disable_proxy()
                .build()
                .expect("shouldn't fail creating OpenSearch-Transport");
            opensearch::OpenSearch::new(transport)
        })
        .await;
    debug!("Successfully initialized OpenSearch-Client.");
    client
}

/// Marker type representing the OpenSearch service in Floci-based tests.
///
/// Implements the `IntegrationTestService` trait to support lifecycle management
/// when used with the `#[aura_integration_test]` macro.
///
pub struct OpenSearch();

#[async_trait]
impl IntegrationTestService for OpenSearch {
    fn service_names(&self) -> &'static [&'static str] {
        &["opensearch"]
    }

    async fn set_up(&self) {
        if let Some(context) = crate::TestContext::current() {
            if crate::environment::managed_environment().is_none() {
                OPENSEARCH_BOOTSTRAPPED
                    .get_or_init(|| async { set_up_open_search().await })
                    .await;
            }
            let index = context
                .product_listings_index()
                .expect("scoped product index");
            context
                .persist_resources()
                .await
                .expect("persist scoped index");
            ensure_index_exists(
                get_opensearch_client().await,
                &index,
                PRODUCT_LISTINGS_INDEX_MAPPING_STR,
            )
            .await
            .expect("create scoped product index");
            return;
        }
        if let Some(config) = crate::environment::managed_environment() {
            assert!(
                config.opensearch_endpoint().is_some(),
                "managed profile has no OpenSearch"
            );
            return;
        }
        OPENSEARCH_BOOTSTRAPPED
            .get_or_init(|| async { set_up_open_search().await })
            .await;
    }

    async fn tear_down(&self) {
        if crate::TestContext::current().is_some() {
            return;
        }
        clear_all_indices().await;
    }
}

async fn domain_endpoint() -> Url {
    if let Some(config) = crate::environment::managed_environment() {
        return Url::parse(
            config
                .opensearch_endpoint()
                .expect("managed profile has no OpenSearch"),
        )
        .expect("managed endpoint was validated");
    }
    // Floci reports a container IP in DescribeDomain. Host clients must use the published port.
    let container_name = format!(
        "floci-aws-{}-opensearch-000000000000-eu-central-1-{TEST_DOMAIN_NAME}",
        aws_emulator_container_name()
    );
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{(index (index .NetworkSettings.Ports \"9200/tcp\") 0).HostPort}}",
            &container_name,
        ])
        .output()
        .await
        .expect("shouldn't fail inspecting OpenSearch port binding");
    assert!(
        output.status.success(),
        "OpenSearch container must have a published port"
    );
    let port = String::from_utf8(output.stdout)
        .expect("OpenSearch port binding must be UTF-8")
        .trim()
        .parse::<u16>()
        .expect("OpenSearch port binding must be a TCP port");
    let mut endpoint = Url::parse(get_endpoint_url()).expect("AWS emulator endpoint must be a URL");
    endpoint
        .set_port(Some(port))
        .expect("OpenSearch endpoint must accept a port");
    endpoint
}

async fn set_up_open_search() {
    let started = std::time::Instant::now();
    set_up_domain()
        .await
        .expect("shouldn't fail creating OpenSearch-Domain");
    wait_until_domain_processed(TEST_DOMAIN_NAME)
        .await
        .expect("shouldn't fail waiting for domain  to complete processing");
    bootstrap_indices(get_opensearch_client().await)
        .await
        .expect("shouldn't fail setting up indices");
    debug!(
        elapsed_ms = started.elapsed().as_millis(),
        "OpenSearch structural bootstrap complete."
    );
    crate::benchmark::record_timing("OPENSEARCH_STARTUP", "PROCESS", started.elapsed());
}

async fn set_up_domain() -> Result<(), BoxError> {
    aws_sdk_opensearch::Client::new(get_aws_config().await)
        .create_domain()
        .domain_name(TEST_DOMAIN_NAME)
        .engine_version("OpenSearch_3.5")
        .access_policies(test_access_policies())
        .send()
        .await
        .map(|_| ())
        .map_err(box_error)
}

async fn wait_until_domain_processed(domain: &'static str) -> Result<(), BoxError> {
    let mut retries = 500;
    let mut processing = true;
    while processing {
        let res = aws_sdk_opensearch::Client::new(get_aws_config().await)
            .describe_domain()
            .domain_name(domain)
            .send()
            .await?;
        if res
            .clone()
            .domain_status
            .expect("shouldn't miss 'domain_status'")
            .processing
            .expect("shouldn't miss 'domain_status.processing'")
        {
            retries -= 1;
            debug!(
                remaining_retries = retries,
                domain = domain,
                "Domain is still being processed..."
            );
            if retries < 0 {
                return Err(std::io::Error::other("Domain took too long to process").into());
            }
            sleep(Duration::from_millis(500)).await;
        } else {
            debug!(
                remaining_retries = retries,
                domain = domain,
                "Domain finished processing."
            );
            processing = false;
        }
    }
    Ok(())
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
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        match set_up_indices(client).await {
            Ok(()) => return Ok(()),
            Err(err) if std::time::Instant::now() < deadline => {
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
    let started = std::time::Instant::now();
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
    crate::benchmark::record_timing("OPENSEARCH_RESET", "PROCESS", started.elapsed());
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
