use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product::watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_watchlist_api::handler;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

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

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    let product_watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_dynamodb_repository,
        &get_product_service,
    );

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &product_watchlist_service).await
        },
    ))
    .await
}
