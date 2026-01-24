use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use aws_sdk_dynamodb::Client;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::command_service::CommandShopServiceImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::query_service::QueryShopServiceImpl;
use shop_api::handler;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let dynamodb = Client::new(&aws_config);
    let shop_dynamodb_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_dynamodb_repository);

    let opensearch = common::opensearch::client::load_client().await?;
    let shop_opensearch_repository = ShopOpenSearchRepositoryImpl::new(&opensearch);
    let query_shop_service = QueryShopServiceImpl::new(&shop_opensearch_repository);

    let command_shop_service = CommandShopServiceImpl::new(&shop_dynamodb_repository);

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(
                event,
                &get_shop_service,
                &query_shop_service,
                &command_shop_service,
            )
            .await
        },
    ))
    .await
}
