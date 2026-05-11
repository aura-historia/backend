use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use common::price::domain::FixedFxRate;
use lambda_runtime::tracing::debug;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
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
    let shop_repository = Box::leak(Box::new(ShopDynamoDbRepositoryImpl::new(
        &dynamodb,
        &table_name,
    )));
    let get_shop_service = Box::leak(Box::new(GetShopServiceImpl::new(shop_repository)));
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &table_name);
    let fx_rate = FixedFxRate();

    let seller_service: Box<dyn SellerService + Sync> = match std::env::var("LOCALSTACK_HOSTNAME") {
        Ok(_) => Box::new(MockSellerService::default()),
        Err(_) => {
            let opensearch = Box::leak(Box::new(
                common::opensearch::client::load_client()
                    .await
                    .expect("shouldn't fail loading OpenSearch-Client"),
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
                    .api_key(&llm_api_key)
                    .model("gemini-2.5-flash"),
            )
            .expect("shouldn't fail creating SellerServiceImpl");
            Box::new(service)
        }
    };

    let product_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        get_shop_service,
        seller_service.as_ref(),
    );

    debug!("Lambda initialized.");

    run(service_fn(|event: LambdaEvent<SqsEvent>| async {
        handler(event, get_shop_service, &product_service).await
    }))
    .await
}
