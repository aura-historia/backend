use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product::watchlist::service::product_watchlist_service::ProductWatchListServiceImpl;
use product_api_watchlist_delete::handler;
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

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let item_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let get_product_service = GetProductServiceImpl::new(&item_repository);

    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &item_repository,
        &get_product_service,
    );

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
