use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use notification_api::handler;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

    let notification_repository = NotificationDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        "noreply@example.com"
            .parse()
            .expect("shouldn't fail parsing placeholder sender email"),
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &notification_service).await
        },
    ))
    .await
}
