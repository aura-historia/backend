use aws_config::BehaviorVersion;
use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::price::domain::FixedFxRate;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use serde_json::Value;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::{MockSellerService, SellerService};
use shopify_lambda::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fx_rate = FixedFxRate();
    let seller_service: Box<dyn SellerService + Sync> = Box::new(MockSellerService::default());
    let product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        &get_shop_service,
        seller_service.as_ref(),
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<EventBridgeEvent<Value>>| async {
            handler(event, &get_shop_service, &product_service).await
        },
    ))
    .await
}
