use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CloudFormationOutput {
    #[serde(rename = "CognitoHostedUIDomain")]
    pub cognito_hosted_ui_domain: String,
    pub cognito_user_pool_id: String,
    pub cognito_user_pool_client_public_id: String,
    pub cognito_user_pool_client_admin_id: String,
    pub api_gateway_endpoint_url: String,
    pub opensearch_domain_endpoint_url: String,
    pub dynamodb_table_1_name: String,
    pub send_mail_queue_url: String,
    pub send_mail_dead_letter_queue_url: String,
    pub item_ingest_events_dynamodb_queue_url: String,
    pub item_ingest_events_dynamodb_dead_letter_queue_url: String,
    pub item_materialize_dynamodb_new_queue_url: String,
    pub item_materialize_dynamodb_new_dead_letter_queue_url: String,
    pub item_materialize_dynamodb_update_queue_url: String,
    pub item_materialize_dynamodb_update_dead_letter_queue_url: String,
    pub item_materialize_opensearch_new_queue_url: String,
    pub item_materialize_opensearch_new_dead_letter_queue_url: String,
    pub item_materialize_opensearch_update_queue_url: String,
    pub item_materialize_opensearch_update_dead_letter_queue_url: String,
    pub item_update_notify_user_queue_url: String,
    pub item_update_notify_user_dead_letter_queue_url: String,
    pub product_enrichment_queue_url: String,
    pub product_enrichment_dead_letter_queue_url: String,
}

static CFN_OUTPUT: OnceLock<CloudFormationOutput> = OnceLock::new();
pub fn get_cfn_output() -> &'static CloudFormationOutput {
    CFN_OUTPUT.get_or_init(|| {
        let json = std::env::var("CFN_OUTPUT").expect("should have CFN_OUTPUT set in CI");
        serde_json::from_str::<CloudFormationOutput>(&json)
            .expect("shouldn't fail deserializing '$CFN_OUTPUT' to 'CloudFormationOutput'")
    })
}
