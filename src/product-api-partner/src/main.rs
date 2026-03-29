use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::price::domain::FixedFxRate;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_api_partner::handler;
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl;
use product_classification::category::opensearch_repository::CategoryOpenSearchRepositoryImpl;
use product_classification::category::service::CategoryServiceImpl;
use product_classification::period::dynamodb_repository::PeriodDynamoDbRepositoryImpl;
use product_classification::period::opensearch_repository::PeriodOpenSearchRepositoryImpl;
use product_classification::period::service::PeriodServiceImpl;
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
    let opensearch = common::opensearch::client::load_client()
        .await
        .expect("shouldn't fail loading OpenSearch-Client");

    let shop_dynamodb_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_dynamodb_repository);

    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fx_rate = FixedFxRate();
    let period_dynamodb_repository = PeriodDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let category_dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let period_opensearch_repository = PeriodOpenSearchRepositoryImpl::new(&opensearch);
    let category_opensearch_repository = CategoryOpenSearchRepositoryImpl::new(&opensearch);
    let period_service =
        PeriodServiceImpl::new(&period_dynamodb_repository, &period_opensearch_repository);
    let category_service = CategoryServiceImpl::new(
        &category_dynamodb_repository,
        &category_opensearch_repository,
    );
    let command_product_service = CommandProductServiceImpl::new(
        &product_dynamodb_repository,
        &fx_rate,
        &period_service,
        &category_service,
    );

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &get_shop_service, &command_product_service).await
        },
    ))
    .await
}
