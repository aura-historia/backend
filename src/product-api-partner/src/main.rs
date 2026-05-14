use aws_config::BehaviorVersion;
use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::slug_id::SlugId;
use fxrate::dynamodb::repository::FxRateDynamoDbRepositoryImpl;
use fxrate::service::FxRateServiceImpl;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use llm::builder::{LLMBackend, LLMBuilder};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_api_partner::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::command_service::CommandShopServiceImpl;
use shop::service::geocoding_service::{
    GeocodingService, GoogleGeocodingService, NoopGeocodingService,
};
use shop::service::get_service::GetShopServiceImpl;
use shop::service::query_service::QueryShopServiceImpl;
use shop::service::seller_service::{MockSellerService, SellerService, SellerServiceImpl};

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

    let shop_opensearch_repository = ShopOpenSearchRepositoryImpl::new(&opensearch);
    let geocoding_service: Box<dyn GeocodingService + Sync> =
        match std::env::var("LOCALSTACK_HOSTNAME") {
            Ok(_) => Box::new(NoopGeocodingService),
            Err(_) => Box::new(GoogleGeocodingService::from_env()?),
        };
    let command_shop_service =
        CommandShopServiceImpl::new(&shop_dynamodb_repository, geocoding_service.as_ref());
    let query_shop_service = QueryShopServiceImpl::new(&shop_opensearch_repository);

    let seller_service: Box<dyn SellerService + Sync> = match std::env::var("LOCALSTACK_HOSTNAME") {
        Ok(_) => {
            let mut mock = MockSellerService::default();
            mock.expect_get_seller_shop_details().returning(|_| {
                Box::pin(async {
                    Ok((
                        ShopId::new(),
                        SlugId::raw("hans-im-glueck"),
                        ShopName::from("Hans im Glück"),
                    ))
                })
            });
            Box::new(mock)
        }
        Err(_) => {
            let llm_api_key = std::env::var("GEMINI_API_KEY")
                .expect("shouldn't fail loading env-var 'GEMINI_API_KEY'");
            let seller_service = SellerServiceImpl::new(
                &shop_dynamodb_repository,
                &get_shop_service,
                &query_shop_service,
                &command_shop_service,
                LLMBuilder::new()
                    .backend(LLMBackend::Google)
                    .api_key(&llm_api_key)
                    .model("gemini-2.5-flash"),
            )
            .expect("shouldn't fail creating SellerServiceImpl");
            Box::new(seller_service)
        }
    };

    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fxrate_service = FxRateServiceImpl::new_read_only(&fxrate_repository);
    let command_product_service = CommandProductServiceImpl::new(
        &product_dynamodb_repository,
        &fxrate_service,
        &get_shop_service,
        seller_service.as_ref(),
    )
    .await?;

    debug!("Lambda initialized.");

    run(service_fn(
        |event: LambdaEvent<ApiGatewayV2httpRequest>| async {
            handler(event, &get_shop_service, &command_product_service).await
        },
    ))
    .await
}
