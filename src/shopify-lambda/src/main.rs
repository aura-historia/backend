use aws_config::BehaviorVersion;
use aws_lambda_events::eventbridge::EventBridgeEvent;
use common::price::domain::FixedFxRate;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
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
    let mut seller_service = MockSellerService::default();
    seller_service.expect_get_seller_shop_details().returning(|_| {
        Box::pin(async {
            Ok((
                ShopId::new(),
                SlugId::raw("shopify-seller"),
                ShopName::from("Shopify Seller"),
            ))
        })
    });
    let seller_service: Box<dyn SellerService + Sync> = Box::new(seller_service);
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
