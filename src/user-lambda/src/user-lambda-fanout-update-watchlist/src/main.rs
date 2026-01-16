use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use tracing::info;
use user_lambda_fanout_update_watchlist::handler;

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

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let client = Client::new(&aws_config);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(&client, &table_name);

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&watchlist_repository, event).await
    }))
    .await
}
