use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use fxrate::dynamodb::repository::FxRateDynamoDbRepositoryImpl;
use fxrate::service::FxRateServiceImpl;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use llm::backends::google::GooglePlatform;
use llm::builder::{LLMBackend, LLMBuilder};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::command_service::CommandShopServiceImpl;
use shop::service::geocoding_service::{GeocodingService, GoogleGeocodingService};
use shop::service::get_service::GetShopServiceImpl;
use shop::service::query_service::QueryShopServiceImpl;
use shop::service::seller_service::{MockSellerService, SellerService, SellerServiceImpl};
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

    let seller_service: Box<dyn SellerService + Sync> = match std::env::var("LOCALSTACK_HOSTNAME") {
        Ok(_) => Box::new(MockSellerService::default()),
        Err(_) => {
            let opensearch = Box::leak(Box::new(
                common::opensearch::client::load_client()
                    .await
                    .expect("shouldn't fail loading OpenSearch-Client (check OPENSEARCH_ENDPOINT_URL and network access)"),
            ));
            let shop_opensearch_repository =
                Box::leak(Box::new(ShopOpenSearchRepositoryImpl::new(opensearch)));
            let geocoding_service: &'static (dyn GeocodingService + Sync) =
                Box::leak(Box::new(GoogleGeocodingService::from_env()?));
            let command_shop_service = Box::leak(Box::new(CommandShopServiceImpl::new(
                shop_repository,
                geocoding_service,
            )));
            let query_shop_service = Box::leak(Box::new(QueryShopServiceImpl::new(
                shop_opensearch_repository,
            )));
            let llm_api_key = std::env::var("GEMINI_API_KEY")
                .expect("shouldn't fail loading env-var 'GEMINI_API_KEY'");
            let service = SellerServiceImpl::new(
                shop_repository,
                get_shop_service,
                query_shop_service,
                command_shop_service,
                LLMBuilder::new()
                    .backend(LLMBackend::Google)
                    .google_platform(GooglePlatform::GeminiEnterpriseAgent {
                        project_id: "aura-historia".to_owned(),
                        region: Some("europe-west3".to_owned()),
                    })
                    .api_key(&llm_api_key)
                    .model("gemini-2.5-flash"),
            )
            .expect("shouldn't fail creating SellerServiceImpl");
            Box::new(service)
        }
    };

    let fxrate_repository = FxRateDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fxrate_service = FxRateServiceImpl::new_read_only(&fxrate_repository);
    let product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fxrate_service,
        get_shop_service,
        seller_service.as_ref(),
    )
    .await?;

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(event, get_shop_service, &product_service).await
    }))
    .await
}
