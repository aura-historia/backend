use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::opensearch::repository::ProductOpenSearchRepositoryImpl;
use product_lambda_delete_product::handler;
use product_watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl;
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let dynamodb_table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let dynamodb_client = Client::new(&aws_config);
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &dynamodb_table_name);
    let search_filter_repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(&dynamodb_client, &dynamodb_table_name);

    let opensearch_client = common::opensearch::client::load_client().await?;
    let opensearch_repository = ProductOpenSearchRepositoryImpl::new(&opensearch_client);

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(
            &opensearch_repository,
            &watchlist_repository,
            &search_filter_repository,
            event,
        )
        .await
    }))
    .await
}
