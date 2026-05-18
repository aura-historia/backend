use aws_config::BehaviorVersion;
use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_dynamodb::Client;
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_pipeline_embed_text::{handler, service::MultimodalEmbeddingServiceImpl};
use tracing::debug;

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
    let command_service =
        CommandProductServiceImpl::new_for_enrichment_pipeline(&product_repository);

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
        let gemini_api_key = std::env::var("GEMINI_API_KEY")
            .expect("shouldn't fail loading env-var 'GEMINI_API_KEY'");
        let embedding_service = MultimodalEmbeddingServiceImpl::new(&gemini_api_key);
        run(service_fn(|event: LambdaEvent<SqsEvent>| async {
            handler(&embedding_service, &command_service, event).await
        }))
        .await
    }
}
