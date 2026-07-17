use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use fxrate::dynamodb::repository::FxRateDynamoDbRepositoryImpl;
use fxrate::service::FxRateServiceImpl;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
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
    // Box::leak is used throughout this initialization block to satisfy the
    // 'static lifetime bounds required by the `service_fn` closure. Lambda
    // processes run for the entire lifetime of the process, so the memory is
    // never reclaimed, but that is acceptable here.
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
        handler(event, get_shop_service, &product_service).await
    }))
    .await
}
