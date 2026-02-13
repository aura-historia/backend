use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use mail_core::queue_service::QueueMailServiceImpl;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::{
    dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_lambda_update_notify_user::{handler, service::ProductEventMailPayloadServiceImpl};
use serde_email::Email;
use tracing::debug;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let sqs_client = aws_sdk_sqs::Client::new(&aws_config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let mail_queue_url =
        std::env::var("MAIL_QUEUE_URL").expect("shouldn't fail loading env-var 'MAIL_QUEUE_URL'");
    let queue_mail_service = QueueMailServiceImpl::new(&sqs_client, &mail_queue_url);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);

    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let sender_mail_str =
        std::env::var("SENDER_MAIL").expect("shouldn't fail loading env-var 'SENDER_MAIL'");
    let sender_mail = Email::try_from(sender_mail_str)?;
    let product_event_mail_payload_service = ProductEventMailPayloadServiceImpl::new(
        &watchlist_service,
        &get_product_service,
        sender_mail,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(
            &queue_mail_service,
            &product_event_mail_payload_service,
            event,
        )
        .await
    }))
    .await
}
