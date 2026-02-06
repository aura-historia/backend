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
use product_pipeline_translate::{
    adapter::TranslationAdapterImpl, process::TranslationPipeProcesserImpl,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

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

    let translate_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let translate_processor =
        TranslationPipeProcesserImpl::new(Arc::new(TranslationAdapterImpl::new().unwrap()));
    let translate_flow_out = PipeFlowOutImpl::new(&product_dynamodb_repository);
    let translate_pipe = PipeImpl::new(
        &get_product_service,
        &sqs,
        source_queue_url,
        256,
        900,
        &translate_flow_in,
        &translate_processor,
        &translate_flow_out,
    );
    translate_pipe.pipe().await;
}
