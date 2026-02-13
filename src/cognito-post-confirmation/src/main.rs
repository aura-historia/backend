use aws_config::BehaviorVersion;
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use aws_sdk_dynamodb::Client;
use cognito_post_confirmation::handler;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
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
    let client = Client::new(&aws_config);
    let repository = UserDynamoDbRepositoryImpl::new(&client, &table_name);
    let service = UserServiceImpl::new(&repository);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>| async {
            handler(event, &service).await
        },
    ))
    .await
}
