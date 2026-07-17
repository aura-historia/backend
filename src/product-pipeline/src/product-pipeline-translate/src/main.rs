use aws_config::BehaviorVersion;
use common::language::domain::Language;
use common::price::domain::FixedFxRate;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::MockFxRateService;
use lambda_runtime::service_fn;
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_pipeline_translate::handler;
use shop::dynamodb::repository::ShopDynamoDbRepositoryImpl;
use shop::service::get_service::GetShopServiceImpl;
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
    let shop_repository = ShopDynamoDbRepositoryImpl::new(&dynamodb, &dynamodb_table_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    // The translate pipeline never performs price conversions, so a fixed FX rate suffices.
    let mut fx_rate_service = MockFxRateService::new();
    fx_rate_service
        .expect_get_current()
        .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
    let command_service =
        CommandProductServiceImpl::new(&product_repository, &fx_rate_service, &get_shop_service)
            .await
            .expect("shouldn't fail initializing CommandProductService");

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
            handler(&translation_service, &command_service, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    } else {
        let gemini_api_key = std::env::var("GEMINI_API_KEY")
            .expect("shouldn't fail reading env-var 'GEMINI_API_KEY'");
        let translation_service =
            product_pipeline_translate::service::TranslationServiceImpl::new(&gemini_api_key);
        lambda_runtime::run(service_fn(|event| {
            handler(&translation_service, &command_service, event)
        }))
        .await
        .expect("shouldn't fail running Lambda");
    }
}
