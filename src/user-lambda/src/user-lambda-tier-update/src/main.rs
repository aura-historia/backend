use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use tracing::debug;
use user_lambda_tier_update::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let client = aws_sdk_dynamodb::Client::new(&aws_config);

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let search_filter_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(&client, &table_name);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&watchlist_repository, &search_filter_repository, event).await
    }))
    .await
}
