use aws_tests_common::get_cfn_output;
use staging_tests::{get_dynamodb_client, get_opensearch_client};

async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _cfn = get_cfn_output();
    let _dynamodb = get_dynamodb_client().await;
    let _opensearch = get_opensearch_client().await;

    todo!()
}
