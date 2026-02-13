use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl;
use product_classification::category::opensearch_repository::CategoryOpenSearchRepositoryImpl;
use product_classification::category::service::CategoryServiceImpl;
use product_classification_api::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb = Client::new(&aws_config);
    let category_dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let opensearch = common::opensearch::client::load_client().await?;
    let category_opensearch_repository = CategoryOpenSearchRepositoryImpl::new(&opensearch);

    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &category_service).await
        },
    ))
    .await
}
