use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::{
    dynamodb::repository::ProductDynamoDbRepositoryImpl,
    opensearch::repository::ProductOpenSearchRepositoryImpl,
};
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl;
use product_lambda_materialize_opensearch::handler;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
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

    let dynamodb_table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb_client = Client::new(&aws_config);
    let dynamodb_repository =
        ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &dynamodb_table_name);
    let category_repository =
        CategoryDynamoDbRepositoryImpl::new(&dynamodb_client, &dynamodb_table_name);

    let opensearch_client = common::opensearch::client::load_client().await?;
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch_client);

    info!(
        dynamoDbTableName = %dynamodb_table_name,
        "Lambda cold start completed, clients initialized."
    );

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(
            &opensearch_repository,
            &dynamodb_repository,
            &category_repository,
            event,
        )
        .await
    }))
    .await
}
