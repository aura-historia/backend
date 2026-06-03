use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use common::price::domain::FixedFxRate;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_pipeline_embed_text::{handler, service::MultimodalEmbeddingServiceImpl};
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
use shop::service::seller_service::MockSellerService;
use tracing::debug;

const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "aura-historia";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";

#[tokio::main]
async fn main() -> Result<(), Error> {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;

    let table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail loading env-var 'DYNAMODB_TABLE_NAME'");

    let client = Client::new(&aws_config);
    let product_repository = ProductDynamoDbRepositoryImpl::new(&client, &table_name);
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&client, &table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    // The embed-text pipeline never performs price conversions, so a fixed FX rate suffices.
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let seller_service = MockSellerService::default();
    let command_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate_service,
        &get_shop_service,
        &seller_service,
    )
    .await
    .expect("shouldn't fail initializing CommandProductService");

    debug!("Lambda initialized.");

    if std::env::var("LOCALSTACK_HOSTNAME").is_ok() {
        use product_pipeline_embed_text::service::MockMultimodalEmbeddingService;

        let mut embedding_service = MockMultimodalEmbeddingService::new();
        embedding_service
            .expect_embed()
            .returning(|_, _, _| Box::pin(async { Ok(vec![0.42f32; 768]) }));
        run(service_fn(|event: LambdaEvent<SqsEvent>| async {
            handler(&embedding_service, &command_service, event).await
        }))
        .await
    } else {
        let _google_application_credentials = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .expect("shouldn't fail loading env-var 'GOOGLE_APPLICATION_CREDENTIALS'");
        let vertex_ai_project_id = std::env::var("VERTEX_AI_PROJECT_ID")
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_PROJECT_ID.to_string());
        let vertex_ai_location = std::env::var("VERTEX_AI_LOCATION")
            .unwrap_or_else(|_| DEFAULT_VERTEX_AI_LOCATION.to_string());
        let embedding_service =
            MultimodalEmbeddingServiceImpl::new(&vertex_ai_project_id, &vertex_ai_location);
        run(service_fn(|event: LambdaEvent<SqsEvent>| async {
            handler(&embedding_service, &command_service, event).await
        }))
        .await
    }
}
