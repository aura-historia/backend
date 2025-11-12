use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::price::domain::FixedFxRate;
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::enrichment_service::ItemCommandEnrichmentServiceImpl;
use product::service::upsert_service::UpsertItemsServiceImpl;
use product_api_put_products::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .without_time()
        .init();

    let aws_config = aws_config::defaults(BehaviorVersion::v2025_08_07())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")?;
    let ingest_item_events_queue_url = std::env::var("INGEST_PRODUCT_EVENTS_QUEUE_URL")?;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let item_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let sqs_client = aws_sdk_sqs::Client::new(&aws_config);
    let fx_rate = FixedFxRate();
    let upsert_service = UpsertItemsServiceImpl::new(
        &item_repository,
        &sqs_client,
        &ingest_item_events_queue_url,
        &fx_rate,
    );
    let enrichment_service = ItemCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);

    info!(
        dynamoDbTableName = %table_name,
        "Lambda cold start completed, client initialized."
    );

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &upsert_service, &enrichment_service).await
        },
    ))
    .await
}
