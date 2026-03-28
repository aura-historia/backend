use aws_config::BehaviorVersion;
use product::{
    dynamodb::repository::ProductDynamoDbRepositoryImpl,
    service::get_service::GetProductServiceImpl,
};
use product_classification::{
    category::{
        dynamodb_repository::CategoryDynamoDbRepositoryImpl,
        opensearch_repository::CategoryOpenSearchRepositoryImpl, service::CategoryServiceImpl,
    },
    period::{
        dynamodb_repository::PeriodDynamoDbRepositoryImpl,
        opensearch_repository::PeriodOpenSearchRepositoryImpl, service::PeriodServiceImpl,
    },
};
use product_pipeline_classify::{
    adapter::ClassifyAdapterImpl, process::ClassifyPipeProcessorImpl,
};
use product_pipeline_common::{
    flow_in::PipeFlowInImpl,
    flow_out::PipeFlowOutImpl,
    pipe::{Pipe, PipeImpl},
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

    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let product_dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);

    let category_dynamodb_repository =
        CategoryDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let category_opensearch_repository = CategoryOpenSearchRepositoryImpl::new(&opensearch);
    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );

    let period_dynamodb_repository =
        PeriodDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let period_opensearch_repository = PeriodOpenSearchRepositoryImpl::new(&opensearch);
    let period_service =
        PeriodServiceImpl::new(&period_dynamodb_repository, &period_opensearch_repository);

    let classify_flow_in = PipeFlowInImpl::new(&sqs, &source_queue_url);
    let classify_processor = ClassifyPipeProcessorImpl::new(
        Arc::new(
            ClassifyAdapterImpl::new().expect("shouldn't fail creating ClassifyAdapterImpl"),
        ),
        &category_service,
        &period_service,
    );
    let classify_flow_out = PipeFlowOutImpl::new(&product_dynamodb_repository);
    let classify_pipe = PipeImpl::new(
        &get_product_service,
        &sqs,
        source_queue_url,
        64,
        300,
        &classify_flow_in,
        &classify_processor,
        &classify_flow_out,
    );
    classify_pipe.pipe().await;
}
