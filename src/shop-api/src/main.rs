use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use cognito::load_access_token_verifier_service;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::command_service::CommandShopServiceImpl;
use shop::service::geocoding_service::{
    GeocodingService, GoogleGeocodingService, NoopGeocodingService,
};
use shop::service::get_service::GetShopServiceImpl;
use shop::service::query_service::QueryShopServiceImpl;
use shop_api::handler;
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

    let dynamodb = Client::new(&aws_config);
    let shop_dynamodb_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_dynamodb_repository);
    let geocoding_service: Box<dyn GeocodingService + Sync> =
        match std::env::var("LOCALSTACK_HOSTNAME") {
            Ok(_) => Box::new(NoopGeocodingService),
            Err(_) => Box::new(GoogleGeocodingService::from_env()?),
        };
    let command_shop_service =
        CommandShopServiceImpl::new(&shop_dynamodb_repository, geocoding_service.as_ref());

    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);

    let opensearch = common::opensearch::client::load_client().await?;
    let shop_opensearch_repository = ShopOpenSearchRepositoryImpl::new(&opensearch);
    let query_shop_service = QueryShopServiceImpl::new(&shop_opensearch_repository);

    let access_token_verifier_service =
        load_access_token_verifier_service(&user_pool_id, &user_pool_client_ids);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &get_shop_service,
                &query_shop_service,
                &command_shop_service,
                &user_service,
                &access_token_verifier_service,
            )
            .await
        },
    ))
    .await
}
