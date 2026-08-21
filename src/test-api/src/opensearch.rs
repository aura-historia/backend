use crate::IntegrationTestService;
use crate::localstack::{LOCALSTACK_CONTAINER_PORT, get_aws_config, get_endpoint_url};
use async_trait::async_trait;
use common::error::boxed::BoxError;

use aws_sdk_opensearch::operation::create_domain::CreateDomainOutput;

use aws_sdk_opensearch::types::DomainEndpointOptions;
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

/// Returns a shared `opensearch::OpenSearch`-Client for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_opensearch_client() -> &'static Client {
    let client = OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let endpoint_url = Url::parse(&format!("{}/{TEST_DOMAIN_NAME}", get_endpoint_url()))
                .expect("shouldn't fail parsing OpenSearch endpoint URL");
            let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
                .build()
                .expect("shouldn't fail creating OpenSearch-Transport");
            opensearch::OpenSearch::new(transport)
        })
        .await;
    debug!("Successfully initialized OpenSearch-Client.");
    client
}

/// Marker type representing the OpenSearch service in LocalStack-based tests.
///
/// Implements the `IntegrationTestService` trait to support lifecycle management
/// when used with the `#[aura_integration_test]` macro.
///
/// ### Dependencies
///
/// LocalStack requires **S3** to be activated when using OpenSearch.
/// You need to supply S3 manually with `#[aura_integration_test(services = [OpenSearch, S3])]`
pub struct OpenSearch();

#[async_trait]
impl IntegrationTestService for OpenSearch {
    fn service_names(&self) -> &'static [&'static str] {
        &["opensearch", "s3"]
    }

    async fn set_up(&self) {
        set_up_open_search(false).await;
    }

    async fn tear_down(&self) {
        clear_all_indices().await;
    }
}

fn test_domain_access_policy() -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": "*",
                "Action": "es:ESHttp*",
                "Resource": format!(
                    "arn:aws:es:eu-central-1:000000000000:domain/{TEST_DOMAIN_NAME}/*"
                )
            }
        ]
    })
    .to_string()
}

#[cfg(feature = "cloudformation")]
pub(crate) async fn set_up_after_cloudformation() {
    set_up_open_search(true).await;
}

async fn set_up_open_search(recreate_existing_domain: bool) {
    set_up_domain(recreate_existing_domain)
        .await
        .expect("shouldn't fail creating OpenSearch-Domain");
    wait_until_domain_processed(TEST_DOMAIN_NAME)
        .await
        .expect("shouldn't fail waiting for domain  to complete processing");
    wait_until_indices_are_set_up()
        .await
        .expect("shouldn't fail setting up indices");
}

async fn set_up_domain(recreate_existing_domain: bool) -> Result<CreateDomainOutput, BoxError> {
    let client = aws_sdk_opensearch::Client::new(get_aws_config().await);
    let custom_endpoint =
        format!("http://localhost:{LOCALSTACK_CONTAINER_PORT}/{TEST_DOMAIN_NAME}");
    let access_policy = test_domain_access_policy();

    match client
        .describe_domain()
        .domain_name(TEST_DOMAIN_NAME)
        .send()
        .await
    {
        Ok(_response) if recreate_existing_domain => {
            debug!(
                "OpenSearch domain '{}' exists from CloudFormation; recreating it for LocalStack",
                TEST_DOMAIN_NAME
            );
            let _ = client
                .delete_domain()
                .domain_name(TEST_DOMAIN_NAME)
                .send()
                .await;
            wait_until_domain_deleted(TEST_DOMAIN_NAME)
                .await
                .expect("shouldn't fail waiting for OpenSearch domain deletion");
        }
        Ok(_response) => {
            // Domain already exists — it may have been created without path-based routing
            // registered. Call update_domain_config to ensure LocalStack routes /test-domain/*.
            debug!(
                "OpenSearch domain '{}' already exists; updating custom endpoint to '{}'",
                TEST_DOMAIN_NAME, custom_endpoint
            );
            let update_result = client
                .update_domain_config()
                .domain_name(TEST_DOMAIN_NAME)
                .access_policies(&access_policy)
                .domain_endpoint_options(
                    DomainEndpointOptions::builder()
                        .custom_endpoint(&custom_endpoint)
                        .custom_endpoint_enabled(true)
                        .build(),
                )
                .access_policies(test_access_policies())
                .send()
                .await;
            match update_result {
                Ok(_) => debug!(
                    "Custom endpoint for '{}' updated successfully.",
                    TEST_DOMAIN_NAME
                ),
                Err(e) => debug!(
                    "Could not update custom endpoint for '{}' (may already be set): {e}",
                    TEST_DOMAIN_NAME
                ),
            }
            return Ok(CreateDomainOutput::builder().build());
        }
        Err(_) => {
            debug!(
                "OpenSearch domain '{}' does not exist, creating it",
                TEST_DOMAIN_NAME
            );
        }
    }

    client
        .create_domain()
        .domain_name(TEST_DOMAIN_NAME)
        .access_policies(access_policy)
        .domain_endpoint_options(
            DomainEndpointOptions::builder()
                // Must use the container-internal port (not the host-mapped port) so that
                // LocalStack can resolve this URL from inside the container when registering
                // the domain. The OpenSearch client uses get_endpoint_url() for host access.
                .custom_endpoint(custom_endpoint)
                .custom_endpoint_enabled(true)
                .build(),
        )
        .access_policies(test_access_policies())
        .send()
        .await
        .map_err(common::error::boxed::box_error)
}

async fn wait_until_domain_deleted(domain: &'static str) -> Result<(), BoxError> {
    let mut retries = 100;
    loop {
        match aws_sdk_opensearch::Client::new(get_aws_config().await)
            .describe_domain()
            .domain_name(domain)
            .send()
            .await
        {
            Ok(_) => {
                retries -= 1;
                if retries < 0 {
                    return Err(std::io::Error::other("Domain took too long to delete").into());
                }
                sleep(Duration::from_millis(500)).await;
            }
            Err(_) => return Ok(()),
        }
    }
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

    mapping
}

static SHOPS_INDEX_MAPPING_STR: &str = include_str!(concat!(
    env!("CARGO_WORKSPACE_DIR"),
    "opensearch/mappings/shops.json"
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

async fn ensure_index_exists(
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

    refresh_index(index).await;
    Ok(())
}

/// Registers the hybrid search pipeline on the local OpenSearch cluster.
///
/// Uses [`HYBRID_SEARCH_PIPELINE_NAME`] as the pipeline ID so it matches what
/// [`product::opensearch::repository::hybrid_search_product_documents`] references via the
/// `search_pipeline` query parameter.
///
/// Registers a `score-ranker-processor` pipeline using Reciprocal Rank Fusion (RRF) —
/// the same native hybrid-fusion technique used in production.
///
/// Panics if the pipeline cannot be registered, so hybrid tests fail fast with a meaningful
/// message rather than surfacing a confusing "pipeline not defined" 400 error later.
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
                "score-ranker-processor RRF pipeline registration returned non-success"
            );
            panic!(
                "Failed to register hybrid search pipeline '{HYBRID_SEARCH_PIPELINE_NAME}' with score-ranker-processor: HTTP {status}",
                status = resp.status_code()
            );
        }
        Err(e) => {
            debug!(
                error = %e,
                "score-ranker-processor RRF pipeline registration failed"
            );
            panic!(
                "Failed to register hybrid search pipeline '{HYBRID_SEARCH_PIPELINE_NAME}' with score-ranker-processor: {e}"
            );
        }
    };
}

async fn set_up_indices() -> Result<(), Error> {
    let client = get_opensearch_client().await;

    // Register the native-hybrid RRF search pipeline before creating indices.
    register_hybrid_search_pipeline(client).await;

    ensure_index_exists(
        client,
        "products",
        mapping_with_inline_synonyms(PRODUCTS_INDEX_MAPPING_STR),
    )
    .await?;
    ensure_index_exists(
        client,
        "shops",
        serde_json::from_str::<serde_json::Value>(SHOPS_INDEX_MAPPING_STR)
            .expect("shouldn't fail parsing SHOPS_INDEX_MAPPING_STR as serde_json::Value"),
    )
    .await?;
    ensure_index_exists(
        client,
        "user_search_filters",
        mapping_with_inline_synonyms(USER_SEARCH_FILTER_INDEX_MAPPING_STR),
    )
    .await?;
    ensure_index_exists(
        client,
        "users",
        serde_json::from_str::<serde_json::Value>(USERS_INDEX_MAPPING_STR)
            .expect("shouldn't fail parsing USERS_INDEX_MAPPING_STR as serde_json::Value"),
    )
    .await?;

    Ok(())
}

async fn wait_until_indices_are_set_up() -> Result<(), Error> {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        match set_up_indices().await {
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
/// test has caused its creation). Reusable from any `IntegrationTestService`
/// implementation that needs a full OpenSearch reset, including the
/// `Cloudformation` service.
pub(crate) async fn clear_all_indices() {
    const INDICES: &[&str] = &["products", "shops", "user_search_filters", "users"];
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
