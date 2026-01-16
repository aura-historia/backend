use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::price::domain::FixedFxRate;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::dynamodb::repository::{FxRateDynamoDbRepository, FxRateDynamoDbRepositoryImpl};
use lambda_runtime::tracing::info;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::enrichment_service::ProductCommandEnrichmentServiceImpl;
use product::service::upsert_service::UpsertProductsServiceImpl;
use product_api_put_products::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use tracing::{error, warn};

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
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb_client, &table_name);
    let fx_rate = fxrate_repository
        .get_fx_rates_record()
        .await
        .unwrap_or_else(|err| {
            error!(error = ?err, "Failed loading FxRate from DynamoDB. Defaulting to FixedFxRate.");
            Some(FxRatesRecord::from(FixedFxRate()))
        })
        .unwrap_or_else(|| {
            warn!("There was no FxRatesRecord in DynamoDB. Defaulting to FixedFxRate.");
            FxRatesRecord::from(FixedFxRate())
        });
    let upsert_service = UpsertProductsServiceImpl::new(&product_repository, &fx_rate);
    let enrichment_service = ProductCommandEnrichmentServiceImpl::new(&shop_repository, &fx_rate);

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
