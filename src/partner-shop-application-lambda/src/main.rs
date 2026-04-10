use aws_config::BehaviorVersion;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::notification_service::NotificationServiceImpl;
use notification::service::s3_adapter::S3AdapterImpl;
use notification::service::ses_adapter::SesAdapterImpl;
use partner_shop_application::dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl;
use partner_shop_application_lambda::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::command_service::CommandShopServiceImpl;
use tracing::debug;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let s3_bucket_name_templates = std::env::var("S3_BUCKET_NAME_TEMPLATES")
        .expect("shouldn't fail loading env-var 'S3_BUCKET_NAME_TEMPLATES'");
    let stage_name =
        std::env::var("STAGE_NAME").expect("shouldn't fail loading env-var 'STAGE_NAME'");
    let commit_sha =
        std::env::var("COMMIT_SHA").expect("shouldn't fail loading env-var 'COMMIT_SHA'");

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let ses_client = aws_sdk_sesv2::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let partner_app_repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);

    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let shop_service = CommandShopServiceImpl::new(&shop_repository);

    let notification_repository =
        NotificationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let ses_adapter = SesAdapterImpl::new(&ses_client);
    let s3_adapter = S3AdapterImpl::new(&s3_client);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &ses_adapter,
        &s3_adapter,
        &s3_bucket_name_templates,
        &stage_name,
        &commit_sha,
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<serde_json::Value>| async {
        handler(
            &partner_app_repository,
            &shop_service,
            &shop_repository,
            &notification_service,
            event,
        )
        .await
    }))
    .await
}
