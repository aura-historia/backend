use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use partner_shop_application::dynamodb::repository::PartnerShopApplicationDynamoDbRepositoryImpl;
use partner_shop_application::service::partner_shop_application_service::PartnerShopApplicationServiceImpl;
use partner_shop_application_api::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");

    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);

    let repository =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let service = PartnerShopApplicationServiceImpl::new(&repository);

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async { handler(event, &service).await },
    ))
    .await
}
