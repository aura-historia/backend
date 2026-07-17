use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use fxrate::dynamodb::repository::FxRateDynamoDbRepositoryImpl;
use fxrate::service::FxRateServiceImpl;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_lambda_ingest_partner_products::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");

    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let shop_repository = Box::leak(Box::new(ShopDynamoDbRepositoryImpl::new(
        &dynamodb,
        &table_name,
    )));
    let get_shop_service = Box::leak(Box::new(GetShopServiceImpl::new(shop_repository)));
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);

    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fxrate_service = FxRateServiceImpl::new_read_only(&fxrate_repository);
    let product_service =
        CommandProductServiceImpl::new(&product_repository, &fxrate_service, get_shop_service)
            .await?;

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(event, &product_service).await
    }))
    .await
}
