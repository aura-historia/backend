use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use item_lambda_update_notify_user::{handler, service::ItemEventMailPayloadServiceImpl};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use mail_core::queue_service::QueueMailServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetItemServiceImpl;
use product::watchlist::{
    dynamodb::repository::WatchlistItemDynamoDbRepositoryImpl,
    service::item_watchlist_service::ProductWatchListServiceImpl,
};
use serde_email::Email;
use tracing::info;
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

    let sqs_client = aws_sdk_sqs::Client::new(&aws_config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let mail_queue_url = std::env::var("MAIL_QUEUE_URL")?;
    let queue_mail_service = QueueMailServiceImpl::new(&sqs_client, &mail_queue_url);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let watchlist_repository =
        WatchlistItemDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let item_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);

    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &item_repository,
        &get_item_service,
    );

    let sender_mail_str = std::env::var("SENDER_MAIL")?;
    let sender_mail = Email::try_from(sender_mail_str)?;
    let item_event_mail_payload_service =
        ItemEventMailPayloadServiceImpl::new(&watchlist_service, &get_item_service, sender_mail);

    info!("Lambda cold start completed, client initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(&queue_mail_service, &item_event_mail_payload_service, event).await
    }))
    .await
}
