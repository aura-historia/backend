use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::{
    dynamodb::repository::NotificationDynamoDbRepositoryImpl,
    service::{
        notification_service::NotificationServiceImpl, s3_adapter::S3AdapterImpl,
        ses_adapter::SesAdapterImpl,
    },
};
use product::{
    dynamodb::repository::ProductDynamoDbRepositoryImpl,
    service::get_service::GetProductServiceImpl,
};
use search_filter::{
    opensearch::repository::UserSearchFilterOpenSearchRepositoryImpl,
    service::user_search_filter_service::UserSearchFilterServiceImpl,
};
use search_filter_lambda_percolate_product::{
    handler, service::ProductEventSearchFilterNotificationsServiceImpl,
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

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let sender_email: serde_email::Email = std::env::var("SENDER_MAIL")
        .expect("shouldn't fail loading env-var 'SENDER_MAIL'")
        .parse()
        .expect("shouldn't fail parsing 'SENDER_MAIL' as email");
    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")
        .expect("shouldn't fail loading env-var 'S3_BUCKET_NAME_TEMPLATES'");
    let stage_name = std::env::var("STAGE_NAME")
        .expect("shouldn't fail loading env-var 'STAGE_NAME'");
    let commit_sha = std::env::var("COMMIT_SHA")
        .expect("shouldn't fail loading env-var 'COMMIT_SHA'");

    let client = aws_sdk_dynamodb::Client::new(&aws_config);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);

    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let ses_adapter = SesAdapterImpl::new(&ses_client);

    let s3_client = aws_sdk_s3::Client::new(&aws_config);
    let s3_adapter = S3AdapterImpl::new(&s3_client);

    let opensearch_client = common::opensearch::client::load_client().await?;
    let search_filter_opensearch_repo =
        UserSearchFilterOpenSearchRepositoryImpl::new(&opensearch_client);

    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let search_filter_dynamodb_repo =
        search_filter::dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl::new(
            &client,
            &table_name,
        );
    let user_search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repo,
        &search_filter_opensearch_repo,
    );
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
    let product_event_search_filter_service =
        ProductEventSearchFilterNotificationsServiceImpl::new(
            &user_search_filter_service,
            &get_product_service,
        );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(
            &product_event_search_filter_service,
            &notification_service,
            event,
        )
        .await
    }))
    .await
}
