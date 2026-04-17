use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use cognito::load_access_token_verifier_service;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use newsletter_api::handler;
use newsletter_api::service::ZohoCampaignsServiceImpl;
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

    let zoho_list_key =
        std::env::var("ZOHO_LIST_KEY").expect("shouldn't fail loading env-var 'ZOHO_LIST_KEY'");
    let zoho_client_id =
        std::env::var("ZOHO_CLIENT_ID").expect("shouldn't fail loading env-var 'ZOHO_CLIENT_ID'");
    let zoho_client_secret = std::env::var("ZOHO_CLIENT_SECRET")
        .expect("shouldn't fail loading env-var 'ZOHO_CLIENT_SECRET'");
    let zoho_refresh_token = std::env::var("ZOHO_REFRESH_TOKEN")
        .expect("shouldn't fail loading env-var 'ZOHO_REFRESH_TOKEN'");
    let zoho_accounts_url = std::env::var("ZOHO_ACCOUNTS_URL")
        .expect("shouldn't fail loading env-var 'ZOHO_ACCOUNTS_URL'");
    let zoho_campaigns_url = std::env::var("ZOHO_CAMPAIGNS_URL")
        .expect("shouldn't fail loading env-var 'ZOHO_CAMPAIGNS_URL'");

    let user_pool_id =
        std::env::var("USER_POOL_ID").expect("shouldn't fail loading env-var 'USER_POOL_ID'");
    let user_pool_public_client_id = std::env::var("USER_POOL_PUBLIC_CLIENT_ID")
        .expect("shouldn't fail loading env-var 'USER_POOL_PUBLIC_CLIENT_ID'");
    let user_pool_admin_client_id = std::env::var("USER_POOL_ADMIN_CLIENT_ID")
        .expect("shouldn't fail loading env-var 'USER_POOL_ADMIN_CLIENT_ID'");
    let user_pool_client_ids = [
        user_pool_public_client_id.as_str(),
        user_pool_admin_client_id.as_str(),
    ];

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);

    let zoho_campaigns_service = ZohoCampaignsServiceImpl::new(
        zoho_list_key,
        reqwest::Client::new(),
        zoho_client_id,
        zoho_client_secret,
        zoho_refresh_token,
        zoho_accounts_url,
        zoho_campaigns_url,
    );

    let access_token_verifier_service =
        load_access_token_verifier_service(&user_pool_id, &user_pool_client_ids);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &zoho_campaigns_service,
                &access_token_verifier_service,
                &user_service,
            )
            .await
        },
    ))
    .await
}
