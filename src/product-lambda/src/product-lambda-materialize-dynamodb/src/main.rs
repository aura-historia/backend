use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product_lambda_materialize_dynamodb::handler;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let client = Client::new(&aws_config);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);
    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&product_repository, event).await
    }))
    .await
}
