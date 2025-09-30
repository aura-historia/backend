use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use item_api_watchlist_post::handler;
use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use item_service::get_service::GetItemServiceImpl;
use item_watchlist::repository::WatchlistItemDynamoDbRepositoryImpl;
use item_watchlist::service::ItemWatchListServiceImpl;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};

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
        WatchlistItemDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let item_repository = ItemDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let get_item_service = GetItemServiceImpl::new(&item_repository);

    let service =
        ItemWatchListServiceImpl::new(&watchlist_repository, &item_repository, &get_item_service);

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
