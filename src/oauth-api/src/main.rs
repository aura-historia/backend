use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use oauth::dynamodb::repository::OAuthDynamoDbRepositoryImpl;
use oauth::service::oauth_service::OAuthServiceImpl;
use oauth_api::handler;
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
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let oauth_repository = OAuthDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);
    let oauth_service = OAuthServiceImpl::new(&oauth_repository, &user_service);

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &oauth_service).await
        },
    ))
    .await
}
