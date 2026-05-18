use aws_config::BehaviorVersion;
use common::language::domain::Language;
use lambda_runtime::service_fn;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product_pipeline_translate::handler;
use std::collections::HashMap;

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
        use product_pipeline_translate::service::MockTranslationService;

        let mut translation_service = MockTranslationService::new();
        translation_service
            .expect_translate()
            .returning(|titles, _source_lang| {
                let count = titles.len();
                Box::pin(async move {
                    vec![
                        Some(HashMap::from([
                            (Language::De, "Antiker Stuhl".to_string()),
                            (Language::En, "Antique chair".to_string()),
                            (Language::Fr, "Chaise ancienne".to_string()),
                            (Language::Es, "Silla antigua".to_string()),
                            (Language::It, "Sedia antica".to_string()),
                        ]));
                        count
                    ]
                })
            });
        lambda_runtime::run(service_fn(|event| {
            handler(&translation_service, &product_repository, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    } else {
        let gemini_api_key = std::env::var("GEMINI_API_KEY")
            .expect("shouldn't fail reading env-var 'GEMINI_API_KEY'");
        let translation_service =
            product_pipeline_translate::service::TranslationServiceImpl::new(&gemini_api_key);
        lambda_runtime::run(service_fn(|event| {
            handler(&translation_service, &product_repository, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    }
}
