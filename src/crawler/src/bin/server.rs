use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{ScraperServiceImpl, SpiderHtmlFetcher};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::spider::candidate_service::SpiderCandidateServiceImpl;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ShopUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use llm::builder::{LLMBackend, LLMBuilder};
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    common::logging::init_logging();

    info!("Starting Crawler Server");

    // 2. Connect to database
    let db_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
    let pool = PgPool::connect(&db_url)
        .await
        .expect("Failed to connect to database");

    // 3. Register 2 demo shops
    register_demo_shops(&pool).await;

    // 4. Wire dependencies
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".to_string());

    let state_llm_builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);

    let state_mapping_repo = Box::new(ProductStateMappingRepositoryImpl::new(Box::leak(Box::new(
        pool.clone(),
    ))));
    let state_mapping_svc =
        ProductStateMappingServiceImpl::new(state_llm_builder, state_mapping_repo)
            .expect("failed to build ProductStateMappingServiceImpl");

    let normalization_svc = ProductNormalizationServiceImpl::new(Box::new(state_mapping_svc));

    let schema_llm_builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);

    let schema_repo = Box::new(ShopsProductSchemaRepositoryImpl::new(Box::leak(Box::new(
        pool.clone(),
    ))));
    let schema_svc = ProductSchemaServiceImpl::new(schema_llm_builder, schema_repo)
        .expect("failed to build ProductSchemaServiceImpl");

    let scraper_candidates = Box::new(ScraperCandidateServiceImpl::new(pool.clone()));

    let fetcher = Box::new(SpiderHtmlFetcher::new());
    let scraper_svc = Box::new(ScraperServiceImpl::new(
        fetcher,
        Box::new(schema_svc),
        Box::new(normalization_svc),
        Arc::new(ScraperCandidateServiceImpl::new(pool.clone())),
    ));

    let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
    let url_pattern_repo = Box::new(ShopUrlPatternRepositoryImpl::new(pool.clone()));

    let class_llm_builder = LLMBuilder::new()
        .backend(LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);
    let class_svc = Box::new(UrlClassificationServiceImpl::new(class_llm_builder).unwrap());

    let pattern_svc = Box::new(UrlPatternServiceImpl::new(
        Arc::new(*url_pattern_repo),
        class_svc,
    ));

    let spider_config = SpiderServiceConfig {
        db_batch_size: 10,
        ..Default::default()
    };
    let website_spider = Box::new(SpiderImpl::default());

    let spider_svc = Box::new(SpiderServiceImpl::new(
        spider_config,
        website_spider,
        pattern_svc,
        url_metadata_repo.clone(),
    ));

    let spider_candidates = Box::new(SpiderCandidateServiceImpl::new(pool.clone()));

    // 5. Build Cron Job
    let config = CrawlerCronConfig {
        spider_interval: Duration::from_secs(600),
        scraper_interval: Duration::from_secs(60),
        spider_batch_size: 10,
        scraper_batch_size: 20,
        spider_concurrency: 3,
        scraper_concurrency: 10,
        spider_classify_threshold: 200,
    };

    let cron_job = CrawlerCronJob::new(
        config,
        spider_candidates,
        spider_svc,
        scraper_candidates,
        scraper_svc,
    );

    // 6. Run forever
    info!("Crawler Server is fully initialized. Starting background tasks...");
    cron_job.run_loop().await;
}

async fn register_demo_shops(pool: &PgPool) {
    info!("Registering demo shops...");

    // Shop 1: Nostalgie Palast
    let shop1_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO shops (shop_id, url_pattern, created, updated)
         VALUES ($1, $2, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop1_id)
    .bind(r".*/(couchtisch|schrank|stuhl|tisch).*")
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         ON CONFLICT (shop_domain) DO NOTHING",
    )
    .bind(shop1_id)
    .bind("nostalgie-palast.de")
    .execute(pool)
    .await
    .unwrap();

    // Shop 2: Antiquitäten Tübingen
    let shop2_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO shops (shop_id, url_pattern, created, updated)
         VALUES ($1, $2, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop2_id)
    .bind(r".*art-\d+.*")
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         ON CONFLICT (shop_domain) DO NOTHING",
    )
    .bind(shop2_id)
    .bind("antiquitaeten-tuebingen.de")
    .execute(pool)
    .await
    .unwrap();

    info!("Demo shops registered successfully.");
}
