use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product_watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_watchlist_api::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    let product_watchlist_service =
        ProductWatchListServiceImpl::new(&watchlist_repository, &get_product_service);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &product_watchlist_service).await
        },
    ))
    .await
}
