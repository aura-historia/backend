//! Demo binary - showcases end-to-end usage of [`SpiderService`].
//!
//! Uses a hardcoded local Postgres database (`crawler_demo_spider`). Bootstrap with:
//!
//! ```powershell
//! # from src/crawler/
//! .\db-up.ps1
//! .\db-migrate.ps1
//! ```
//!
//! # Configuration
//!
//! | Env var          | Purpose                 | Default                         |
//! |------------------|-----------------------  |---------------------------------|
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL | `.../crawler_demo_spider`      |
//! | `GEMINI_API_KEY` | API key for Gemini      | *(required)*                    |
//! | `GEMINI_MODEL`   | Model name to use       | `gemini-3.1-flash-lite-preview` |
//! | `LOG_LEVEL`      | Log level for this demo | `info`                          |
//!
//! # Running
//!
//! ```powershell
//! $env:GEMINI_API_KEY="..."
//! cargo run --bin demo-spider -p crawler -- https://www.christies.com/en
//! ```

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;

use common::shop_id::ShopId;
use crawler::local_db::{DEMO_SPIDER_DB_NAME, bootstrap_local_database, demo_spider_db_url};
use crawler::spider::SpiderRunResult;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ShopUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceError;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderDiscoveryError;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::{SpiderService, SpiderServiceConfig, SpiderServiceImpl};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Error)]
enum DemoError {
    #[error("Demo error: {0}")]
    Demo(String),

    #[error(transparent)]
    EnvVar(#[from] std::env::VarError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Discovery(#[from] SpiderDiscoveryError),

    #[error(transparent)]
    UrlPattern(#[from] UrlPatternServiceError),

    #[error(transparent)]
    SpiderService(#[from] crawler::spider::SpiderServiceError),
}

const DEFAULT_SHOP_URL: &str = "https://www.christies.com/en";
const DEFAULT_CLASSIFY_THRESHOLD: usize = 200;
/// Spider demo pool size: 1 advisory-lock connection + 4 query connections.
const DEMO_POOL_MAX_CONNECTIONS: u32 = 5;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    let shop_url = read_shop_url();
    let api_key = match read_api_key() {
        Ok(api_key) => api_key,
        Err(error) => {
            error!(error = %error, "Failed to load configuration");
            return;
        }
    };

    // Connect to local Postgres and apply pending migrations.
    let pool = match connect_and_migrate().await {
        Ok(p) => p,
        Err(error) => {
            error!(error = %error, "Failed to connect to Postgres");
            return;
        }
    };

    let pattern_repository = build_pattern_repository(pool.clone());
    let url_repository = build_url_repository(pool.clone());

    let crawler = Box::new(SpiderImpl::default());
    let model =
        env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.1-flash-lite-preview".to_string());
    let llm_builder = llm::builder::LLMBuilder::new()
        .backend(llm::builder::LLMBackend::Google)
        .api_key(&api_key)
        .model(&model);
    let classification_service = Box::new(
        UrlClassificationServiceImpl::new(llm_builder)
            .expect("Failed to initialize UrlClassificationService"),
    );
    let pattern_service = Box::new(UrlPatternServiceImpl::new(
        pattern_repository.clone(),
        classification_service,
    ));

    let spider = SpiderServiceImpl::new(
        SpiderServiceConfig::default(),
        crawler,
        pattern_service,
        url_repository,
    );

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_url_parsed = url::Url::parse(&shop_url)
        .unwrap_or_else(|_| url::Url::parse("https://demo.invalid").unwrap());
    let demo_domain = shop_url_parsed
        .host_str()
        .unwrap_or("demo.invalid")
        .to_string();

    let demo_domain_id = match insert_demo_shop(&pool, &shop_id, &demo_domain).await {
        Ok(id) => id,
        Err(error) => {
            error!(error = %error, "Failed to insert demo shop rows into DB");
            return;
        }
    };

    match spider
        .run(
            &shop_id,
            &demo_domain_id,
            &shop_url,
            DEFAULT_CLASSIFY_THRESHOLD,
        )
        .await
    {
        Ok(result) => {
            info!(
                linkCount = result.total_links,
                productCount = result.product_urls_count,
                "Spider run finished successfully"
            );
            if let Err(error) = write_output(&result) {
                error!(error = %error, "Failed to write demo output file");
            } else {
                info!("Output written to 'spider_output.json'");
            }
        }
        Err(error) => {
            error!(error = %error, "Spider run failed");
        }
    }
}

fn read_shop_url() -> String {
    let raw_url = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_SHOP_URL.to_string());

    ensure_scheme(&raw_url)
}

fn read_api_key() -> Result<String, DemoError> {
    Ok(env::var("GEMINI_API_KEY")?)
}

fn build_pattern_repository(pool: PgPool) -> Arc<ShopUrlPatternRepositoryImpl> {
    Arc::new(ShopUrlPatternRepositoryImpl::new(pool))
}

fn build_url_repository(pool: PgPool) -> Arc<UrlMetadataRepositoryImpl> {
    Arc::new(UrlMetadataRepositoryImpl::new(pool))
}

/// Connects to local Postgres and applies pending migrations.
async fn connect_and_migrate() -> Result<PgPool, DemoError> {
    bootstrap_local_database(DEMO_SPIDER_DB_NAME)
        .await
        .map_err(DemoError::Demo)?;
    let db_url = demo_spider_db_url();

    let pool = PgPoolOptions::new()
        .max_connections(DEMO_POOL_MAX_CONNECTIONS)
        .connect(&db_url)
        .await?;

    info!(
        max_connections = DEMO_POOL_MAX_CONNECTIONS,
        "Connected to Postgres"
    );

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| DemoError::Database(sqlx::Error::from(e)))?;

    info!("Database migrations applied successfully");

    Ok(pool)
}

/// Inserts a demo `shops` row and a `shop_domains` row, returning the generated `domain_id`.
///
/// Uses `ON CONFLICT DO NOTHING` so the function is idempotent if called multiple times
/// with the same `shop_id` / `shop_domain`.
async fn insert_demo_shop(
    pool: &PgPool,
    shop_id: &ShopId,
    shop_domain: &str,
) -> Result<uuid::Uuid, DemoError> {
    let shop_id_uuid: uuid::Uuid = (*shop_id).into();

    sqlx::query(
        "INSERT INTO shops (shop_id, shop_name, shop_slug, shop_type, active, created, updated)
         VALUES ($1, 'Demo Shop', 'demo-shop', 'COMMERCIAL_DEALER', TRUE, NOW(), NOW())
         ON CONFLICT (shop_id) DO NOTHING",
    )
    .bind(shop_id_uuid)
    .execute(pool)
    .await?;

    // Insert the domain row if it doesn't exist yet and return the domain_id.
    // Because `shop_domain` is UNIQUE, a second run with the same domain would hit the conflict
    // path — we return the existing domain_id in that case.
    let domain_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO shop_domains (shop_id, shop_domain, last_crawled)
         VALUES ($1, $2, NULL)
         ON CONFLICT (shop_domain) DO UPDATE SET shop_id = EXCLUDED.shop_id
         RETURNING domain_id",
    )
    .bind(shop_id_uuid)
    .bind(shop_domain)
    .fetch_one(pool)
    .await?;

    Ok(domain_id)
}

fn ensure_scheme(url: &str) -> String {
    let trimmed = url.trim();

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{},spider=warn,sqlx::postgres::notice=warn",
        raw_level
    ));

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}

fn write_output(result: &SpiderRunResult) -> Result<(), std::io::Error> {
    let file = File::create("spider_output.json")?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, result)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}
