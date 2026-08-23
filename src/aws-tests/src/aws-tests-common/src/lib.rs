use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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

pub fn set_cfn_output(output: CloudFormationOutput) {
    CFN_OUTPUT
        .set(output)
        .expect("shouldn't fail setting CFN_OUTPUT; was it already set?");
}
