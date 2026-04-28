use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use opensearch::cluster::ClusterHealthParts;
use opensearch::http::headers::HeaderMap;
use opensearch::http::request::JsonBody;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::http::{Method, StatusCode, Url};
use opensearch::indices::{IndicesExistsParts, IndicesRefreshParts};
use opensearch::{DeleteByQueryParts, Error, GetParts, OpenSearch as Client};
use serde::de::DeserializeOwned;
use serde::ser::Error as SerdeError;
use serde_json::json;
use std::error::Error as StdError;
use std::process::Command;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio::time::sleep;
use tracing::debug;

/// Name of the hybrid search pipeline — must stay in sync with
/// `product::opensearch::repository::HYBRID_SEARCH_PIPELINE_NAME`.
///
/// A direct import is not possible because `product` tests depend on `test-api` as a
/// dev-dependency, making a `test-api → product` dependency circular.
const HYBRID_SEARCH_PIPELINE_NAME: &str = "hybrid-search-pipeline";

pub const TEST_DOMAIN_NAME: &str = "test-domain";

/// A lazily-initialized, globally shared OpenSearch client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static OPENSEARCH_CLIENT: OnceCell<Client> = OnceCell::const_new();
static OPENSEARCH_ENDPOINT_URL: OnceCell<String> = OnceCell::const_new();

type TestResult<T> = Result<T, Box<dyn StdError + Send + Sync>>;

/// Returns a shared `opensearch::OpenSearch` client for the Floci-managed real
/// OpenSearch container.
///
/// Floci only emulates the AWS OpenSearch *management* API on port 4566. In real mode it
/// starts a dedicated OpenSearch container per domain and exposes that container on a host
/// port. Data-plane calls (`/_search`, index creation, pipelines, ... ) must therefore go
/// to the domain endpoint instead of the Floci management endpoint.
pub async fn get_opensearch_client() -> &'static Client {
    let client = OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let endpoint_url = get_opensearch_endpoint_url().await;
            build_opensearch_client(endpoint_url)
        })
        .await;
    debug!("Successfully initialized OpenSearch-Client.");
    client
}

async fn get_opensearch_endpoint_url() -> &'static str {
    OPENSEARCH_ENDPOINT_URL
        .get_or_init(|| async {
            wait_until_domain_ready(TEST_DOMAIN_NAME)
                .await
                .expect("shouldn't fail waiting for OpenSearch domain to become ready")
        })
        .await
        .as_str()
}

fn build_opensearch_client(endpoint_url: &str) -> Client {
    let endpoint_url = Url::parse(endpoint_url).unwrap_or_else(|_| {
        panic!("shouldn't fail parsing OpenSearch endpoint URL '{endpoint_url}'")
    });
    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
        .build()
        .expect("shouldn't fail creating OpenSearch-Transport");
    opensearch::OpenSearch::new(transport)
}

/// Marker type representing the OpenSearch service in Floci-backed tests.
pub struct OpenSearch();

#[async_trait]
impl IntegrationTestService for OpenSearch {
    fn service_names(&self) -> &'static [&'static str] {
        &["opensearch"]
    }

    async fn set_up(&self) {
        set_up_domain()
            .await
            .expect("shouldn't fail creating OpenSearch-Domain");
        set_up_indices()
            .await
            .expect("shouldn't fail setting up indices");
    }

    async fn tear_down(&self) {
        clear_all_indices().await;
    }
}

async fn set_up_domain() -> TestResult<()> {
    let client = aws_sdk_opensearch::Client::new(get_aws_config().await);

    match client
        .describe_domain()
        .domain_name(TEST_DOMAIN_NAME)
        .send()
        .await
    {
        Ok(_) => {
            debug!("OpenSearch domain '{TEST_DOMAIN_NAME}' already exists");
            Ok(())
        }
        Err(_) => {
            debug!("OpenSearch domain '{TEST_DOMAIN_NAME}' does not exist, creating it");
            client
                .create_domain()
                .domain_name(TEST_DOMAIN_NAME)
                .send()
                .await?;
            Ok(())
        }
    }
}

async fn wait_until_domain_ready(domain: &'static str) -> TestResult<String> {
    let mut retries = 500;
    loop {
        if let Some(endpoint_url) = host_reachable_domain_endpoint(domain).await
            && cluster_health_is_ready(&endpoint_url).await
        {
            debug!(
                domain = domain,
                endpoint = endpoint_url,
                "OpenSearch domain is ready"
            );
            return Ok(endpoint_url);
        }

        retries -= 1;
        if retries < 0 {
            return Err(
                format!("OpenSearch domain '{domain}' took too long to become ready").into(),
            );
        }

        debug!(
            remaining_retries = retries,
            domain = domain,
            "OpenSearch domain is not ready yet..."
        );
        sleep(Duration::from_millis(500)).await;
    }
}

async fn host_reachable_domain_endpoint(domain: &str) -> Option<String> {
    let endpoint = aws_sdk_opensearch::Client::new(get_aws_config().await)
        .describe_domain()
        .domain_name(domain)
        .send()
        .await
        .ok()
        .and_then(|response| response.domain_status)
        .and_then(|status| status.endpoint)
        .filter(|endpoint| !endpoint.is_empty())
        .map(|endpoint| normalize_endpoint_url(&endpoint));

    if endpoint.as_deref().is_some_and(is_localhost_endpoint) {
        return endpoint;
    }

    docker_mapped_opensearch_endpoint(domain).or(endpoint)
}

fn normalize_endpoint_url(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_owned()
    } else {
        format!("http://{endpoint}")
    }
}

fn is_localhost_endpoint(endpoint: &str) -> bool {
    Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1"))
}

fn docker_mapped_opensearch_endpoint(domain: &str) -> Option<String> {
    let container_name = format!("floci-opensearch-{domain}");
    let output = Command::new("docker")
        .args(["port", &container_name, "9200/tcp"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let port = stdout
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .filter_map(|port| port.trim().parse::<u16>().ok())
        .next()?;

    Some(format!("http://localhost:{port}"))
}

async fn cluster_health_is_ready(endpoint_url: &str) -> bool {
    let client = build_opensearch_client(endpoint_url);
    match client
        .cluster()
        .health(ClusterHealthParts::None)
        .request_timeout(Duration::from_secs(2))
        .send()
        .await
    {
        Ok(response) if response.status_code().is_success() => {
            let body = response
                .json::<serde_json::Value>()
                .await
                .unwrap_or_default();
            matches!(body["status"].as_str(), Some("green" | "yellow"))
        }
        Ok(response) => {
            debug!(status = %response.status_code(), "OpenSearch cluster health returned non-success");
            false
        }
        Err(error) => {
            debug!(error = %error, "OpenSearch cluster health check failed");
            false
        }
    }
}

static PRODUCTS_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/products.json"
));

static ENGLISH_SYNONYMS_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/analysis/english_synonyms.txt"
));

static GERMAN_SYNONYMS_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/analysis/german_synonyms.txt"
));

static FRENCH_SYNONYMS_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/analysis/french_synonyms.txt"
));

static SPANISH_SYNONYMS_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/analysis/spanish_synonyms.txt"
));

static ITALIAN_SYNONYMS_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/analysis/italian_synonyms.txt"
));

/// Parses synonym file content into a list of synonym rules,
/// filtering out comments and blank lines.
fn parse_synonym_rules(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Converts the products mapping from `synonyms_path` to inline `synonyms`
/// so that LocalStack OpenSearch can create the index without needing
/// synonym files on the cluster filesystem.
fn mapping_with_inline_synonyms(mapping: &'static str) -> serde_json::Value {
    let mut mapping: serde_json::Value = serde_json::from_str(mapping)
        .unwrap_or_else(|_| panic!("shouldn't fail parsing {mapping} as serde_json::Value"));

    let synonym_files = [
        ("english_synonyms", ENGLISH_SYNONYMS_STR),
        ("german_synonyms", GERMAN_SYNONYMS_STR),
        ("french_synonyms", FRENCH_SYNONYMS_STR),
        ("spanish_synonyms", SPANISH_SYNONYMS_STR),
        ("italian_synonyms", ITALIAN_SYNONYMS_STR),
    ];

    for (filter_name, content) in synonym_files {
        let rules = parse_synonym_rules(content);
        if let Some(filter) =
            mapping.pointer_mut(&format!("/settings/analysis/filter/{filter_name}"))
        {
            let obj = filter.as_object_mut().unwrap();
            obj.remove("synonyms_path");
            obj.remove("updateable");
            obj.insert(
                "synonyms".to_owned(),
                serde_json::Value::Array(
                    rules.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
        }
    }

    // OpenSearch 3 requires an explicit `bits` setting for the scalar-quantization
    // encoder. Production mappings are kept as-is; tests patch the mapping in-memory so
    // the same repository tests can run against Floci's OpenSearch 3 image.
    if let Some(encoder_parameters) =
        mapping.pointer_mut("/mappings/properties/embedding/method/parameters/encoder/parameters")
        && let Some(obj) = encoder_parameters.as_object_mut()
        && obj.get("type").and_then(|value| value.as_str()) == Some("fp16")
        && !obj.contains_key("bits")
    {
        obj.insert("bits".to_owned(), json!(16));
    }

    mapping
}

static SHOPS_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/shops.json"
));

static CATEGORIES_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/categories.json"
));

static USER_SEARCH_FILTER_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/user_search_filters.json"
));

static USERS_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/users.json"
));

fn check_status_allow_not_found(response: &Response) -> Result<(), Error> {
    if let Err(err) = response.error_for_status_code_ref()
        && err.status_code() != Some(StatusCode::NOT_FOUND)
    {
        return Err(err);
    }
    Ok(())
}

/// Registers the hybrid search pipeline on the local OpenSearch cluster.
///
/// Uses [`HYBRID_SEARCH_PIPELINE_NAME`] as the pipeline ID so it matches what
/// [`product::opensearch::repository::hybrid_search_product_documents`] references via the
/// `search_pipeline` query parameter.
///
/// Registers a `score-ranker-processor` pipeline using Reciprocal Rank Fusion (RRF),
/// matching the OpenSearch 3 image used by Floci in integration tests.
///
/// Panics if the pipeline cannot be registered, so hybrid tests fail fast with a meaningful
/// message rather than surfacing a confusing "pipeline not defined" 400 error.
async fn register_hybrid_search_pipeline(client: &Client) {
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

    match client
        .send(
            Method::Put,
            &path,
            HeaderMap::new(),
            None::<&serde_json::Value>,
            Some(JsonBody::new(rrf_body)),
            None,
        )
        .await
    {
        Ok(resp) if resp.status_code().is_success() => {
            debug!("Registered hybrid search pipeline '{HYBRID_SEARCH_PIPELINE_NAME}' (RRF)");
            true
        }
        Ok(resp) => {
            debug!(
                status = %resp.status_code(),
                "score-ranker-processor RRF pipeline registration returned non-success; \
                 will attempt normalization-processor fallback"
            );
            panic!(
                "Failed to register hybrid search pipeline '{HYBRID_SEARCH_PIPELINE_NAME}' with score-ranker-processor: HTTP {status}",
                status = resp.status_code()
            );
        }
        Err(e) => {
            debug!(
                error = %e,
                "score-ranker-processor RRF pipeline registration failed; \
                 will attempt normalization-processor fallback"
            );
            panic!(
                "Failed to register hybrid search pipeline '{HYBRID_SEARCH_PIPELINE_NAME}' with score-ranker-processor: {e}"
            );
        }
    };
}

async fn set_up_indices() -> Result<(), Error> {
    let client = get_opensearch_client().await;

    register_hybrid_search_pipeline(client).await;

    create_index_if_missing(
        client,
        "products",
        mapping_with_inline_synonyms(PRODUCTS_INDEX_MAPPING_STR),
    )
    .await?;
    create_index_if_missing(client, "shops", parse_mapping(SHOPS_INDEX_MAPPING_STR)).await?;
    create_index_if_missing(
        client,
        "categories",
        parse_mapping(CATEGORIES_INDEX_MAPPING_STR),
    )
    .await?;
    create_index_if_missing(
        client,
        "user_search_filters",
        mapping_with_inline_synonyms(USER_SEARCH_FILTER_INDEX_MAPPING_STR),
    )
    .await?;
    create_index_if_missing(client, "users", parse_mapping(USERS_INDEX_MAPPING_STR)).await?;

    Ok(())
}

fn parse_mapping(mapping: &'static str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(mapping)
        .unwrap_or_else(|_| panic!("shouldn't fail parsing {mapping} as serde_json::Value"))
}

async fn create_index_if_missing(
    client: &Client,
    index: &'static str,
    mapping: serde_json::Value,
) -> Result<(), Error> {
    let exists_response = client
        .indices()
        .exists(IndicesExistsParts::Index(&[index]))
        .send()
        .await?;
    check_status_allow_not_found(&exists_response)?;

    if exists_response.status_code().is_success() {
        debug!("OpenSearch index '{index}' already exists, skipping creation");
        return Ok(());
    }

    debug!("OpenSearch index '{index}' does not exist, creating it");
    let response = client
        .indices()
        .create(opensearch::indices::IndicesCreateParts::Index(index))
        .body(mapping)
        .send()
        .await?;
    let status = response.status_code();
    let payload = response.text().await?;
    if !status.is_success() {
        return Err(<serde_json::Error as SerdeError>::custom(format!(
            "failed creating OpenSearch index '{index}' with HTTP {status}: {payload}"
        ))
        .into());
    }

    Ok(())
}

/// Clears all documents from every standard index to ensure test isolation.
///
/// Silently skips any index that does not yet exist (e.g. before the first
/// test has caused its creation). Reusable from any `IntegrationTestService`
/// implementation that needs a full OpenSearch reset, including the
/// `Cloudformation` service.
pub(crate) async fn clear_all_indices() {
    const INDICES: &[&str] = &[
        "products",
        "shops",
        "categories",
        "user_search_filters",
        "users",
    ];
    for index in INDICES {
        match clear_index_data(index).await {
            Ok(_) => refresh_index(index).await,
            Err(e) => {
                debug!("Skipping clear for OpenSearch index '{index}' (may not exist yet): {e}")
            }
        }
    }
    debug!("Cleared all OpenSearch indices for test isolation");
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
mod tests {
    use super::*;

    #[test]
    fn should_parse_synonym_rules_when_content_has_comments_and_blank_lines() {
        let content = "# A comment\nsideboard, buffet\n\n# Another comment\nwardrobe, armoire\n";
        let rules = parse_synonym_rules(content);
        assert_eq!(rules, vec!["sideboard, buffet", "wardrobe, armoire"]);
    }

    #[test]
    fn should_return_empty_rules_when_content_is_only_comments() {
        let content = "# Comment only\n# Another comment\n\n";
        let rules = parse_synonym_rules(content);
        assert!(rules.is_empty());
    }

    #[rstest::rstest]
    #[case::product(PRODUCTS_INDEX_MAPPING_STR)]
    #[case::product(USER_SEARCH_FILTER_INDEX_MAPPING_STR)]
    fn should_build_mapping_with_inline_synonyms_for_all_languages(#[case] mapping: &'static str) {
        let mapping = mapping_with_inline_synonyms(mapping);

        let filter_names = [
            "english_synonyms",
            "german_synonyms",
            "french_synonyms",
            "spanish_synonyms",
            "italian_synonyms",
        ];

        for filter_name in filter_names {
            let filter = mapping
                .pointer(&format!("/settings/analysis/filter/{filter_name}"))
                .unwrap_or_else(|| panic!("filter '{filter_name}' should exist"));

            assert!(
                filter.get("synonyms_path").is_none(),
                "'{filter_name}' should not contain 'synonyms_path'"
            );
            assert!(
                filter.get("updateable").is_none(),
                "'{filter_name}' should not contain 'updateable'"
            );

            let synonyms = filter
                .get("synonyms")
                .unwrap_or_else(|| panic!("'{filter_name}' should contain 'synonyms'"));
            let rules = synonyms.as_array().unwrap();

            assert!(
                !rules.is_empty(),
                "'{filter_name}' should have at least one synonym rule"
            );
        }
    }

    #[rstest::rstest]
    #[case::product(PRODUCTS_INDEX_MAPPING_STR)]
    #[case::product(USER_SEARCH_FILTER_INDEX_MAPPING_STR)]
    fn should_set_search_analyzer_on_title_fields_in_products_mapping(
        #[case] mapping: &'static str,
    ) {
        let mapping = mapping_with_inline_synonyms(mapping);

        let title_fields = ["titleEn", "titleDe", "titleFr", "titleEs", "titleIt"];
        let expected_search_analyzers = [
            "english_with_synonyms",
            "german_with_synonyms",
            "french_with_synonyms",
            "spanish_with_synonyms",
            "italian_with_synonyms",
        ];

        for (field, expected_analyzer) in title_fields.iter().zip(expected_search_analyzers.iter())
        {
            let field_mapping = mapping
                .pointer(&format!("/mappings/properties/{field}"))
                .unwrap_or_else(|| panic!("'{field}' should exist"));

            assert_eq!(
                field_mapping["search_analyzer"].as_str().unwrap(),
                *expected_analyzer,
                "'{field}' should use search_analyzer '{expected_analyzer}'"
            );

            assert!(
                field_mapping.get("fields").is_none(),
                "'{field}' should not have sub-fields"
            );
        }
    }
}
