//! Demo binary - showcases end-to-end usage of [`SpiderService`].
//!
//! # Configuration
//!
//! | Env var          | Purpose                 | Default      |
//! |------------------|-------------------------|--------------|
//! | `GEMINI_API_KEY` | API key for Gemini      | *(required)* |
//! | `LOG_LEVEL`      | Log level for this demo | `info`       |
//!
//! # Running
//!
//! ```bash
//! GEMINI_API_KEY=... cargo run --bin demo -p aura-spider -- https://www.christies.com/en
//! ```

use std::env;
use std::fs::File;
use std::io::BufWriter;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use aura_spider::classification::gemini_client::GeminiClient;
use aura_spider::classification::link_metadata_repository::LinkMetadataRepositoryImpl;
use aura_spider::classification::url_pattern_repository::ShopUrlPatternRepositoryImpl;
use aura_spider::classification::url_pattern_service::UrlPatternServiceImpl;
use aura_spider::discovery::website_spider::SpiderCrawler;
use aura_spider::error::SpiderError;
use aura_spider::service::{
    SpiderRunResult, SpiderService, SpiderServiceConfig, SpiderServiceImpl,
};
use sqlx::PgPool;
use testcontainers::ImageExt;
use testcontainers::core::IntoContainerPort;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres as PgImage;
use tracing::{Level, error, info};
use common::shop_id::ShopId;

const DEFAULT_SHOP_URL: &str = "https://www.christies.com/en";
const DEFAULT_CLASSIFY_THRESHOLD: usize = 200;
const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "postgres";
const POSTGRES_DB: &str = "postgres";
const POSTGRES_PORT: u16 = 5432;
const DEMO_CONTAINER_NAME: &str = "aura-historia-spider-demo";

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

    let (_postgres_container, pool) = match start_postgres().await {
        Ok(state) => state,
        Err(error) => {
            error!(error = %error, "Failed to start Postgres for demo");
            return;
        }
    };

    if let Err(error) = apply_schema(&pool).await {
        error!(error = %error, "Failed to apply spider demo schema");
        return;
    }

    let pattern_repository = build_pattern_repository(pool.clone());
    let link_repository = build_link_repository(pool.clone());

    let crawler = Box::new(SpiderCrawler::default());
    let gemini_client = Box::new(GeminiClient::new(api_key));
    let pattern_service = Box::new(UrlPatternServiceImpl::new(
        pattern_repository.clone(),
        gemini_client,
    ));

    let spider = SpiderServiceImpl::new(
        SpiderServiceConfig::default(),
        crawler,
        pattern_service,
        link_repository,
    );

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    match spider.run(&shop_id, &shop_url, DEFAULT_CLASSIFY_THRESHOLD).await {
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

fn read_api_key() -> Result<String, SpiderError> {
    Ok(env::var("GEMINI_API_KEY")?)
}

fn build_pattern_repository(pool: PgPool) -> Arc<ShopUrlPatternRepositoryImpl> {
    Arc::new(ShopUrlPatternRepositoryImpl::new(pool))
}

fn build_link_repository(pool: PgPool) -> Arc<LinkMetadataRepositoryImpl> {
    Arc::new(LinkMetadataRepositoryImpl::new(pool))
}

async fn apply_schema(pool: &PgPool) -> Result<(), SpiderError> {
    let workspace_root = env!("CARGO_WORKSPACE_DIR");
    let sql_path = std::path::Path::new(workspace_root).join("src/aura-spider/sql/schema.sql");

    let sql = std::fs::read_to_string(&sql_path).map_err(SpiderError::Io)?;
    sqlx::raw_sql(&sql).execute(pool).await?;

    info!(path = %sql_path.display(), "Applied spider demo schema");
    Ok(())
}

async fn start_postgres() -> Result<(testcontainers::ContainerAsync<PgImage>, PgPool), SpiderError>
{
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
        .map_err(|error| {
            SpiderError::Spider(format!("Failed to start Postgres container: {error}"))
        })?;

    let connection_string = format!(
        "postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{POSTGRES_PORT}/{POSTGRES_DB}"
    );

    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(100);

    loop {
        attempt += 1;
        match PgPool::connect(&connection_string).await {
            Ok(pool) => {
                info!(attempt, "Connected to Postgres for spider demo");
                return Ok((container, pool));
            }
            Err(error) if attempt < 20 => {
                info!(attempt, error = %error, "Postgres not ready yet, retrying");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(2));
            }
            Err(error) => {
                return Err(SpiderError::Spider(format!(
                    "Could not connect to Postgres after {attempt} attempts: {error}"
                )));
            }
        }
    }
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
    let level = raw_level.parse::<Level>().unwrap_or(Level::INFO);

    tracing_subscriber::fmt().with_max_level(level).init();
}

fn write_output(result: &SpiderRunResult) -> Result<(), std::io::Error> {
    let file = File::create("spider_output.json")?;
    let writer = BufWriter::new(file);
    serde_json::to_writer_pretty(writer, result)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(())
}
