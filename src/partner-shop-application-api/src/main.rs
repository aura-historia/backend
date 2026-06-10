use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use partner_shop_application::dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationServiceImpl;
use partner_shop_application::service::sfn_adapter::SfnAdapterImpl;
use partner_shop_application_api::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
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
    let state_machine_arn = std::env::var("STATE_MACHINE_ARN")
        .expect("shouldn't fail loading env-var 'STATE_MACHINE_ARN'");

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let sfn_client = aws_sdk_sfn::Client::new(&aws_config);

    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let shop_service = GetShopServiceImpl::new(&shop_repository);
    let sfn_adapter = SfnAdapterImpl::new(&sfn_client);
    let service = PartnerShopApplicationServiceImpl::new(
        &repository,
        &shop_service,
        &sfn_adapter,
        &state_machine_arn,
    );

    let user_repository = UserDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let user_service = UserServiceImpl::new(&user_repository);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &service, &user_service).await
        },
    ))
    .await
}
