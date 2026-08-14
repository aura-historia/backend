use aws_sdk_cognitoidentityprovider::types::{AttributeType, AuthFlowType, MessageActionType};
use aws_sdk_dynamodb::types::WriteRequest;
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use common::actor::{RequestContext, domain::Actor};
use fake::Fake;
use fake::faker::internet::de_de::{Password, SafeEmail};
use opensearch::http::response::Response;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::{collections::HashMap, error::Error};
use tokio::sync::OnceCell;
use tracing::debug;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::command::CreateUserCommand;
use user::service::user_service::{UserService, UserServiceImpl};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CloudFormationOutput {
    #[serde(rename = "CognitoHostedUIDomain")]
    pub cognito_hosted_ui_domain: String,
    pub cognito_user_pool_id: String,
    pub cognito_user_pool_client_public_id: String,
    pub api_gateway_endpoint_url: String,
    pub opensearch_endpoint_url: String,
    pub dynamodb_table_1_name: String,
    pub notification_send_queue_url: String,
    pub notification_send_dead_letter_queue_url: String,
    pub product_materialize_opensearch_queue_url: String,
    pub product_materialize_opensearch_dead_letter_queue_url: String,
    pub product_delete_product_queue_url: String,
    pub product_delete_product_dead_letter_queue_url: String,
    pub product_partner_ingest_queue_url: String,
    pub product_partner_ingest_dead_letter_queue_url: String,
    pub shop_opensearch_index_queue_url: String,
    pub shop_opensearch_index_dead_letter_queue_url: String,
    pub user_opensearch_index_queue_url: String,
    pub user_opensearch_index_dead_letter_queue_url: String,
    pub search_filter_open_search_sync_queue_url: String,
    pub search_filter_open_search_sync_dead_letter_queue_url: String,
    pub product_update_notify_user_queue_url: String,
    pub product_update_notify_user_dead_letter_queue_url: String,
    #[serde(default)]
    pub stripe_event_bus_name: String,
    #[serde(default)]
    pub shopify_event_bus_name: String,
}

static CFN_OUTPUT: OnceLock<CloudFormationOutput> = OnceLock::new();
pub fn get_cfn_output() -> &'static CloudFormationOutput {
    CFN_OUTPUT.get_or_init(|| {
        let json = std::env::var("CFN_OUTPUT").expect("should have CFN_OUTPUT set in CI");
        serde_json::from_str::<CloudFormationOutput>(&json)
            .expect("shouldn't fail deserializing '$CFN_OUTPUT' to 'CloudFormationOutput'")
    })
}

/// Sets the CloudFormation output directly (used by LocalStack-based acceptance tests).
///
/// # Panics
///
/// Panics if the output has already been set (i.e., called more than once).
pub fn set_cfn_output(output: CloudFormationOutput) {
    CFN_OUTPUT
        .set(output)
        .expect("shouldn't fail setting CFN_OUTPUT; was it already set?");
}

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
            unsafe {
                std::env::set_var(
                    "OPENSEARCH_ENDPOINT_URL",
                    get_cfn_output().opensearch_endpoint_url.clone(),
                )
            };
            common::opensearch::client::load_client()
                .await
                .expect("shouldn't fail loading OpenSearch-Client")
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

#[allow(clippy::too_many_arguments)]
pub async fn create_test_user(email: &str) -> TestUser {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;
    let password: String = format!("{}*1bC", Password(8..12).fake::<String>());

    let req_builder = cognito
        .admin_create_user()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(email)
        .user_attributes(
            AttributeType::builder()
                .name("email")
                .value(email)
                .build()
                .unwrap(),
        )
        .message_action(MessageActionType::Suppress);

    let created = req_builder.send().await.unwrap();
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
        .initiate_auth()
        .client_id(&cfn.cognito_user_pool_client_public_id)
        .auth_flow(AuthFlowType::UserPasswordAuth)
        .auth_parameters("USERNAME", email)
        .auth_parameters("PASSWORD", &password)
        .send()
        .await
        .unwrap()
        .authentication_result
        .unwrap();

    let sub: Uuid = created
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
        .unwrap();

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let create_user_command = CreateUserCommand {
        id: sub.into(),
        email: email.try_into().unwrap(),
    };
    let _ = user_service
        .create_user(
            &RequestContext {
                actor: Actor::User(sub.into()),
            },
            create_user_command,
        )
        .await
        .unwrap();

    TestUser {
        access_token: auth.access_token.unwrap(),
        id_token: auth.id_token.unwrap(),
        sub,
    }
}

pub async fn reset() {
    let cfn_output = get_cfn_output().clone();
    clear_ddb_table_data()
        .await
        .expect("shouldn't fail clearing table-data");
    clear_os_index_data("products")
        .await
        .expect("shouldn't fail clearing os-index 'products'");
    clear_os_index_data("shops")
        .await
        .expect("shouldn't fail clearing os-index 'shops'");
    clear_os_index_data("user_search_filters")
        .await
        .expect("shouldn't fail clearing os-index 'user_search_filter'");
    clear_qs(vec![
        cfn_output.notification_send_queue_url,
        cfn_output.notification_send_dead_letter_queue_url,
        cfn_output.product_materialize_opensearch_queue_url,
        cfn_output.product_materialize_opensearch_dead_letter_queue_url,
        cfn_output.product_delete_product_queue_url,
        cfn_output.product_delete_product_dead_letter_queue_url,
        cfn_output.shop_opensearch_index_queue_url,
        cfn_output.shop_opensearch_index_dead_letter_queue_url,
        cfn_output.search_filter_open_search_sync_queue_url,
        cfn_output.search_filter_open_search_sync_dead_letter_queue_url,
        cfn_output.product_update_notify_user_queue_url,
        cfn_output.product_update_notify_user_dead_letter_queue_url,
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
        .await?
        .error_for_status_code()?;

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
