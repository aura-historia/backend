use std::env;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

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
use testcontainers::ImageExt;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres as PgImage;
use tracing::{error, info};

const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";
const POSTGRES_PORT: u16 = 5432;
const DEMO_CONTAINER_NAME: &str = "aura-historia-crawler-demo";

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    let api_key = match env::var("OPENAI_API_KEY") {
        Ok(api_key) => api_key,
        Err(e) => {
            error!("Missing OPENAI_API_KEY: {e}. Please set it to run the demo.");
            return;
        }
    };

    let model = std::env::var("OPENAI_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-flash-lite-preview".to_string());

    let (_postgres_container, pool) = match start_postgres().await {
        Ok(state) => state,
        Err(error) => {
            error!(error = %error, "Failed to start Postgres for demo");
            return;
        }
    };

    if let Err(error) = apply_schema(&pool).await {
        error!(error = %error, "Failed to apply crawler demo schema");
        return;
    }

    if let Err(error) = register_demo_shops(&pool).await {
        error!(error = %error, "Failed to register demo shops");
        return;
    }

    info!("Wiring crawler dependencies...");

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
        spider_interval: Duration::from_secs(120), // Demo: retry spider every 2 minutes
        scraper_interval: Duration::from_secs(30), // Demo: run scraper loop every 30 seconds
        spider_batch_size: 5,
        scraper_batch_size: 20,
        spider_concurrency: 3,
        scraper_concurrency: 10,
        spider_classify_threshold: 100,
    };

    let cron_job = CrawlerCronJob::new(
        config,
        spider_candidates,
        spider_svc,
        scraper_candidates,
        scraper_svc,
    );

    info!("Crawler Server is fully initialized. Starting background tasks. Press Ctrl+C to stop.");
    cron_job.run_loop().await;
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(format!("{},spider=warn", raw_level));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}

async fn start_postgres() -> Result<(testcontainers::ContainerAsync<PgImage>, PgPool), String> {
    let _ = Command::new("docker")
        .args(["rm", "-f", DEMO_CONTAINER_NAME])
        .output();

    info!("Starting Postgres container '{DEMO_CONTAINER_NAME}'");

    let container: testcontainers::ContainerAsync<PgImage> = PgImage::default()
        .with_user(POSTGRES_USER)
        .with_password(POSTGRES_PASSWORD)
        .with_db_name(POSTGRES_DB)
        .with_container_name(DEMO_CONTAINER_NAME)
        .with_mapped_port(POSTGRES_PORT, POSTGRES_PORT.tcp())
        .start()
        .await
        .map_err(|error| format!("Failed to start Postgres container: {error}"))?;

    let connection_string = format!(
        "postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{POSTGRES_PORT}/{POSTGRES_DB}"
    );

    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(100);

    loop {
        attempt += 1;
        match PgPool::connect(&connection_string).await {
            Ok(pool) => {
                info!(attempt, "Connected to Postgres for crawler demo");
                return Ok((container, pool));
            }
            Err(error) if attempt < 20 => {
                info!(attempt, error = %error, "Postgres not ready yet, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
            }
            Err(error) => {
                return Err(format!(
                    "Could not connect to Postgres after {attempt} attempts: {error}"
                ));
            }
        }
    }
}

async fn apply_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    let workspace_root = env!("CARGO_WORKSPACE_DIR");
    let sql_path = std::path::Path::new(workspace_root).join("src/crawler/sql/schema.sql");
    let sql = std::fs::read_to_string(&sql_path).map_err(sqlx::Error::Io)?;
    sqlx::raw_sql(&sql).execute(pool).await?;
    info!(path = %sql_path.display(), "Applied crawler demo schema");
    Ok(())
}

async fn register_demo_shops(pool: &PgPool) -> Result<(), sqlx::Error> {
    info!("Registering 3 demo shops...");

    // Shop 1
    let shop1_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO shops (shop_id, url_pattern, created, updated)
         VALUES ($1, $2, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop1_id)
    .bind(r".*/(couchtisch|schrank|stuhl|tisch).*")
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         ON CONFLICT (shop_domain) DO NOTHING",
    )
    .bind(shop1_id)
    .bind("nostalgie-palast.de")
    .execute(pool)
    .await?;

    // Shop 2
    let shop2_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO shops (shop_id, url_pattern, created, updated)
         VALUES ($1, $2, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop2_id)
    .bind(r".*art-\d+.*")
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         ON CONFLICT (shop_domain) DO NOTHING",
    )
    .bind(shop2_id)
    .bind("antiquitaeten-tuebingen.de")
    .execute(pool)
    .await?;

    // Shop 3
    let shop3_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO shops (shop_id, url_pattern, created, updated)
         VALUES ($1, $2, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop3_id)
    .bind(r".*/lot/.*")
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO shop_domains (shop_id, shop_domain)
         VALUES ($1, $2)
         ON CONFLICT (shop_domain) DO NOTHING",
    )
    .bind(shop3_id)
    .bind("antik-shop.de")
    .execute(pool)
    .await?;

    info!("Demo shops registered successfully.");
    Ok(())
}
