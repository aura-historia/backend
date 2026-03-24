use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::service::{s3_adapter::S3AdapterImpl, ses_adapter::SesAdapterImpl};
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
use user::{
    dynamodb::repository::UserDynamoDbRepositoryImpl, service::user_service::UserServiceImpl,
};

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")
        .expect("shouldn't fail loading env-var 'S3_BUCKET_NAME_TEMPLATES'");
    let stage_name =
        std::env::var("STAGE_NAME").expect("shouldn't fail loading env-var 'STAGE_NAME'");
    let commit_sha =
        std::env::var("COMMIT_SHA").expect("shouldn't fail loading env-var 'COMMIT_SHA'");
    let sender_mail =
        std::env::var("SENDER_MAIL").expect("shouldn't fail loading env-var 'SENDER_MAIL'");
    let sender_email: serde_email::Email = sender_mail
        .try_into()
        .expect("shouldn't fail parsing 'SENDER_MAIL' as email address");

    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);

    let ses_adapter = SesAdapterImpl::new(&ses_client);
    let s3_adapter = S3AdapterImpl::new(&s3_client);

    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_service =
        ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);
    let user_service = UserServiceImpl::new(&user_repository);

    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &ses_adapter,
        &s3_adapter,
        &s3_bucket_name_templates,
        &stage_name,
        &commit_sha,
        sender_email,
    );
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
