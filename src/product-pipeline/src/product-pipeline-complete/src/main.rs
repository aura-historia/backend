use aws_config::BehaviorVersion;
use opensearch::http::response::Response;
use product::{
    dynamodb::repository::ProductDynamoDbRepositoryImpl,
    opensearch::repository::ProductOpenSearchRepositoryImpl,
};
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    pipe::{Pipe, PipeImpl},
    types::{CompletedPipeProduct, TextEmbeddedPipeProduct},
};
use product_pipeline_complete::{
    flow_out::PersistDynamoDbOpenSearchPipeFlowOutImpl, process::CompleterPipeProcessorImpl,
};
use serde_json::json;
use tracing::info;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;
    let sqs = aws_sdk_sqs::Client::new(&aws_config);
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let opensearch: opensearch::OpenSearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let source_queue_url = std::env::var("SOURCE_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'SOURCE_QUEUE_URL'");
    let dynamodb_table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail reading env-var 'DYNAMODB_TABLE_NAME'");
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch);

    let _ = opensearch
        .indices()
        .put_settings(opensearch::indices::IndicesPutSettingsParts::Index(&[
            "products",
        ]))
        .body(json!({
            "index": {
                "refresh_interval": "-1"
            }
        }))
        .send()
        .await
        .map(Response::error_for_status_code)
        .expect("shouldn't fail setting refresh-interval to '-1'")
        .expect("shouldn't convert status-code to error as response is expected to be 2xx");
    info!(
        index = "products",
        refreshInterval = "-1",
        "Updated refresh-interval."
    );

    let complete_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let complete_processor = CompleterPipeProcessorImpl();
    let complete_flow_out = PersistDynamoDbOpenSearchPipeFlowOutImpl::new(
        &product_dynamodb_repository,
        &product_opensearch_repository,
    );
    let complete_pipe: PipeImpl<
        '_,
        TextEmbeddedPipeProduct,
        TextEmbeddedPipeProduct,
        CompletedPipeProduct,
        CompletedPipeProduct,
    > = PipeImpl::new(
        &sqs,
        source_queue_url,
        256,
        300,
        &complete_flow_in,
        &complete_processor,
        &complete_flow_out,
    );
    complete_pipe.pipe().await;

    let _ = opensearch
        .indices()
        .put_settings(opensearch::indices::IndicesPutSettingsParts::Index(&[
            "products",
        ]))
        .body(json!({
            "index": {
                "refresh_interval": "5m"
            }
        }))
        .send()
        .await
        .map(Response::error_for_status_code)
        .expect("shouldn't fail setting refresh-interval to '5m'")
        .expect("shouldn't convert status-code to error as response is expected to be 2xx");
    info!(
        index = "products",
        refreshInterval = "5m",
        "Updated refresh-interval."
    );
}
