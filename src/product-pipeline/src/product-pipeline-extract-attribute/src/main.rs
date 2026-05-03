use aws_config::BehaviorVersion;
use lambda_runtime::service_fn;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product_pipeline_extract_attribute::handler;

#[tokio::main]
async fn main() {
    common::logging::init_logging();

    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);
    let dynamodb_table_name = std::env::var("DYNAMODB_TABLE_NAME")
        .expect("shouldn't fail reading env-var 'DYNAMODB_TABLE_NAME'");
    let product_repository = ProductDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);

    if std::env::var("LOCALSTACK_HOSTNAME").is_ok() {
        use product_pipeline_extract_attribute::service::MockExtractionService;
        use product_pipeline_extract_attribute::types::ExtractedAttributes;

        let mut extraction_service = MockExtractionService::new();
        extraction_service.expect_extract().returning(|texts| {
            let count = texts.len();
            Box::pin(async move {
                vec![
                    Some(ExtractedAttributes {
                        y: Some(1900.into()),
                        nazi: Some(false),
                        ..Default::default()
                    });
                    count
                ]
            })
        });
        lambda_runtime::run(service_fn(|event| {
            handler(&extraction_service, &product_repository, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    } else {
        let gemini_api_key = std::env::var("GEMINI_API_KEY")
            .expect("shouldn't fail reading env-var 'GEMINI_API_KEY'");
        let extraction_service =
            product_pipeline_extract_attribute::service::ExtractionServiceImpl::new(
                &gemini_api_key,
            );
        lambda_runtime::run(service_fn(|event| {
            handler(&extraction_service, &product_repository, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    }
}
