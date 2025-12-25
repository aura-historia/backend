use aws_config::BehaviorVersion;
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
}
