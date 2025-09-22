use aws_sdk_cognitoidentityprovider::types::{AuthFlowType, MessageActionType};
use aws_sdk_dynamodb::types::WriteRequest;
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use aws_tests_common::get_cfn_output;
use fake::Fake;
use fake::faker::internet::de_de::{Password, SafeEmail};
use opensearch::http::Url;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
pub use staging_tests_macros::staging_test;
use std::{collections::HashMap, error::Error};
use tokio::sync::OnceCell;
use tracing::debug;
use uuid::Uuid;

static CONFIG: OnceCell<aws_config::SdkConfig> = OnceCell::const_new();
pub async fn get_aws_config() -> &'static aws_config::SdkConfig {
    CONFIG
        .get_or_init(|| async {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_max_level(tracing::Level::INFO)
                .with_current_span(true)
                .with_ansi(false)
                .try_init();
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await
        })
        .await
}

static DYNAMODB_CLIENT: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();
pub async fn get_dynamodb_client() -> &'static aws_sdk_dynamodb::Client {
    DYNAMODB_CLIENT
        .get_or_init(|| async { aws_sdk_dynamodb::Client::new(get_aws_config().await) })
        .await
}

static OPENSEARCH_CLIENT: OnceCell<opensearch::OpenSearch> = OnceCell::const_new();
pub async fn get_opensearch_client() -> &'static opensearch::OpenSearch {
    OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let transport = TransportBuilder::new(SingleNodeConnectionPool::new(
                Url::parse(&get_cfn_output().opensearch_domain_endpoint_url)
                    .expect("shouldn't fail parsing 'opensearch_domain_endpoint_url' as URL"),
            ))
            .auth(
                get_aws_config()
                    .await
                    .clone()
                    .try_into()
                    .expect("shouldn't fail extracting AWS-Config for OpenSearch"),
            )
            .service_name("es")
            .build()
            .expect("shouldn't fail creating OpenSearch-Transport");
            opensearch::OpenSearch::new(transport)
        })
        .await
}

static SQS_CLIENT: OnceCell<aws_sdk_sqs::Client> = OnceCell::const_new();
pub async fn get_sqs_client() -> &'static aws_sdk_sqs::Client {
    SQS_CLIENT
        .get_or_init(|| async { aws_sdk_sqs::Client::new(get_aws_config().await) })
        .await
}

static COGNITO_CLIENT: OnceCell<aws_sdk_cognitoidentityprovider::Client> = OnceCell::const_new();
pub async fn get_cognito_client() -> &'static aws_sdk_cognitoidentityprovider::Client {
    COGNITO_CLIENT
        .get_or_init(|| async {
            aws_sdk_cognitoidentityprovider::Client::new(get_aws_config().await)
        })
        .await
}

pub struct TestUser {
    pub access_token: String,
    pub id_token: String,
    pub sub: Uuid,
}
pub async fn create_random_test_user() -> TestUser {
    let email: String = SafeEmail().fake();
    create_test_user(&email).await
}

pub async fn create_test_user(email: &str) -> TestUser {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;
    let password: String = format!("{}*1bC", Password(8..12).fake::<String>());

    let created = cognito
        .admin_create_user()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(email)
        .message_action(MessageActionType::Suppress)
        .send()
        .await
        .unwrap();
    cognito
        .admin_set_user_password()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(email)
        .password(&password)
        .permanent(true)
        .send()
        .await
        .unwrap();
    let auth = cognito
        .admin_initiate_auth()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .client_id(&cfn.cognito_user_pool_client_admin_id)
        .auth_flow(AuthFlowType::AdminUserPasswordAuth)
        .auth_parameters("USERNAME", email)
        .auth_parameters("PASSWORD", &password)
        .send()
        .await
        .unwrap()
        .authentication_result
        .unwrap();

    TestUser {
        access_token: auth.access_token.unwrap(),
        id_token: auth.id_token.unwrap(),
        sub: created
            .user
            .unwrap()
            .attributes
            .unwrap()
            .into_iter()
            .find(|attr| attr.name == "sub")
            .unwrap()
            .value
            .unwrap()
            .try_into()
            .unwrap(),
    }
}

// Called inside the macro
pub async fn reset() {
    let cfn_output = get_cfn_output().clone();
    clear_ddb_table_data()
        .await
        .expect("shouldn't fail clearing table-data");
    clear_os_index_data("items")
        .await
        .expect("shouldn't fail clearing os-index 'items'");
    clear_os_index_data("shops")
        .await
        .expect("shouldn't fail clearing os-index 'items'");
    clear_qs(vec![
        cfn_output.item_ingest_events_dynamodb_queue_url,
        cfn_output.item_ingest_events_dynamodb_dead_letter_queue_url,
        cfn_output.item_materialize_dynamodb_new_queue_url,
        cfn_output.item_materialize_dynamodb_new_dead_letter_queue_url,
        cfn_output.item_materialize_dynamodb_update_queue_url,
        cfn_output.item_materialize_dynamodb_update_dead_letter_queue_url,
        cfn_output.item_materialize_opensearch_new_queue_url,
        cfn_output.item_materialize_opensearch_new_dead_letter_queue_url,
        cfn_output.item_materialize_opensearch_update_queue_url,
        cfn_output.item_materialize_opensearch_update_dead_letter_queue_url,
    ])
    .await
    .expect("shouldn't fail clearing queues");
    clear_cognito()
        .await
        .expect("shouldn't fail clearing cognito");
}

/// Clears all items from the DynamoDB table to ensure test isolation.
///
/// This function scans the table and deletes all items in batches.
async fn clear_ddb_table_data() -> Result<(), Box<dyn Error>> {
    use aws_sdk_dynamodb::types::{AttributeValue, DeleteRequest};

    let client = get_dynamodb_client().await;

    // Scan the table to get all items
    let mut exclusive_start_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let mut scan_request = client
            .scan()
            .table_name(get_cfn_output().dynamodb_table_1_name.clone());

        if let Some(start_key) = exclusive_start_key {
            scan_request = scan_request.set_exclusive_start_key(Some(start_key));
        }

        let scan_output = scan_request.consistent_read(true).send().await?;

        if let Some(items) = scan_output.items
            && !items.is_empty()
        {
            // Delete items in batches
            let delete_requests: Vec<WriteRequest> = items
                .into_iter()
                .map(|item| {
                    let mut key = HashMap::new();
                    key.insert("pk".to_string(), item.get("pk").unwrap().clone());
                    key.insert("sk".to_string(), item.get("sk").unwrap().clone());

                    WriteRequest::builder()
                        .delete_request(
                            DeleteRequest::builder().set_key(Some(key)).build().unwrap(),
                        )
                        .build()
                })
                .collect();

            // Process deletes in batches of 25 (DynamoDB limit)
            for chunk in delete_requests.chunks(25) {
                let mut request_items = HashMap::new();
                request_items.insert(
                    get_cfn_output().dynamodb_table_1_name.clone(),
                    chunk.to_vec(),
                );

                client
                    .batch_write_item()
                    .set_request_items(Some(request_items))
                    .send()
                    .await?;
                debug!("Cleared a chunk of size '{}' from table", chunk.len());
            }
        }

        // Check if there are more items to scan
        exclusive_start_key = scan_output.last_evaluated_key;
        if exclusive_start_key.is_none() {
            break;
        }
    }

    debug!(
        "Cleared table '{}'.",
        get_cfn_output().dynamodb_table_1_name
    );

    Ok(())
}

async fn clear_os_index_data(index: &str) -> Result<Response, opensearch::Error> {
    use opensearch::DeleteByQueryParts;
    use serde_json::json;

    let query = json!({
        "query": {
            "match_all": {}
        }
    });

    let res = get_opensearch_client()
        .await
        .delete_by_query(DeleteByQueryParts::Index(&[index]))
        .body(query)
        .refresh(true)
        .send()
        .await?;

    debug!("Cleared index '{index}'.");

    Ok(res)
}

// Manually deleting in batches as purging introduces 60s no-op window
async fn clear_q(queue_url: String) -> Result<(), Box<dyn Error>> {
    let client = get_sqs_client().await;
    loop {
        let resp = client
            .receive_message()
            .queue_url(queue_url.clone())
            .max_number_of_messages(10)
            .wait_time_seconds(1)
            .send()
            .await?;

        let messages = resp.messages.unwrap_or_default();
        if messages.is_empty() {
            break;
        }

        let entries: Vec<_> = messages
            .into_iter()
            .filter_map(|m| {
                m.receipt_handle.map(|handle| {
                    DeleteMessageBatchRequestEntry::builder()
                        .id(uuid::Uuid::new_v4().to_string())
                        .receipt_handle(handle)
                        .build()
                        .unwrap()
                })
            })
            .collect();

        client
            .delete_message_batch()
            .queue_url(queue_url.clone())
            .set_entries(Some(entries.clone()))
            .send()
            .await?;
        debug!(
            "Removed batch of size '{}' from queue '{}'.",
            entries.len(),
            queue_url
        );
    }

    debug!("Cleared queue '{queue_url}'.");

    Ok(())
}

async fn clear_qs(queue_urls: Vec<String>) -> Result<(), Box<dyn Error>> {
    for queue_url in queue_urls {
        clear_q(queue_url).await?;
    }
    Ok(())
}

async fn clear_cognito() -> Result<(), Box<dyn Error>> {
    let client = get_cognito_client().await;
    let mut pagination_token = None;

    loop {
        let resp = client
            .list_users()
            .user_pool_id(&get_cfn_output().cognito_user_pool_id)
            .set_pagination_token(pagination_token.clone())
            .limit(60) // max page size
            .send()
            .await?;

        for u in resp.users() {
            if let Some(username) = u.username() {
                client
                    .admin_delete_user()
                    .user_pool_id(&get_cfn_output().cognito_user_pool_id)
                    .username(username)
                    .send()
                    .await?;
            }
        }

        pagination_token = resp.pagination_token().map(|s| s.to_string());
        if pagination_token.is_none() {
            break;
        }
    }

    Ok(())
}
