use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::opensearch::repository::UserOpenSearchRepositoryImpl;
use user::service::cognito_admin_service::cognito_impl::CognitoAdminServiceImpl;
use user::service::user_service::UserServiceImpl;
use user_api::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");
    let user_pool_id = std::env::var("COGNITO_USER_POOL_ID")
        .expect("shouldn't fail loading env-var 'COGNITO_USER_POOL_ID'");

    let dynamodb_client = Client::new(&aws_config);
    let cognito_client = aws_sdk_cognitoidentityprovider::Client::new(&aws_config);
    let opensearch_client = common::opensearch::client::load_client().await?;

    let repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let cognito_admin_service = CognitoAdminServiceImpl::new(&cognito_client, &user_pool_id);
    let opensearch_repository = UserOpenSearchRepositoryImpl::new(&opensearch_client);
    let service = UserServiceImpl::with_cognito_and_opensearch(
        &repository,
        &cognito_admin_service,
        &opensearch_repository,
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
