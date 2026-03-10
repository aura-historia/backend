use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::{
    dynamodb::repository::NotificationDynamoDbRepositoryImpl,
    service::notification_service::NotificationServiceImpl,
};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product_lambda_update_notify_user::{
    handler, service::ProductEventWatchlistNotificationsServiceImpl,
};
use product_watchlist::{
    dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use tracing::debug;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);

    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &product_repository,
        &get_product_service,
    );

    let notification_service = NotificationServiceImpl::new(&notification_repository);
    let product_event_mail_payload_service = ProductEventWatchlistNotificationsServiceImpl::new(
        &watchlist_service,
        &get_product_service,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(
            &product_event_mail_payload_service,
            &notification_service,
            event,
        )
        .await
    }))
    .await
}
