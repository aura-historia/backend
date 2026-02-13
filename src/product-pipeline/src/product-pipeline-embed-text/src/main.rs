use aws_config::BehaviorVersion;
use product::{
    dynamodb::repository::ProductDynamoDbRepositoryImpl,
    service::get_service::GetProductServiceImpl,
};
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
};
use product_pipeline_embed_text::{
    adapter::EmbeddingAdapterImpl, process::TextEmbeddingPipeProcesserImpl,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let sqs = aws_sdk_sqs::Client::new(&aws_config);
    let source_queue_url = std::env::var("SOURCE_QUEUE_URL")
        .expect("shouldn't fail reading env-var 'SOURCE_QUEUE_URL'");
    let dynamodb_table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail reading env-var 'DYNAMODB_TABLE_NAME'");

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);

    let embed_text_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let embed_text_processor =
        TextEmbeddingPipeProcesserImpl::new(Arc::new(EmbeddingAdapterImpl::new().unwrap()));
    let embed_text_flow_out = PipeFlowOutImpl::new(&product_dynamodb_repository);
    let embed_text_pipe = PipeImpl::new(
        &get_product_service,
        &sqs,
        source_queue_url,
        256,
        900,
        &embed_text_flow_in,
        &embed_text_processor,
        &embed_text_flow_out,
    );
    embed_text_pipe.pipe().await;
}
