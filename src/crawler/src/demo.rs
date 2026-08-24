//! Demo binary — runs the full crawler pipeline (spider + scraper) against a set of hardcoded
//! antique shops without needing a running shop-service or DynamoDB.
//!
//! On startup the demo automatically runs `docker compose up -d` (using the
//! `docker-compose.yml` inside the `crawler` crate) and waits for Postgres to
//! become ready before applying migrations. No manual setup required — just:
//!
//! ```powershell
//! gcloud auth application-default login
//! $env:VERTEX_AI_PROJECT_ID="my-project"
//! $env:VERTEX_AI_LOCATION="europe-west3"
//! cargo run -p crawler --bin demo
//! ```
//!
//! # Configuration
//!
//! | Env var          | Purpose                              | Default                                          |
//! |------------------|--------------------------------------|--------------------------------------------------|
//! | `VERTEX_AI_PROJECT_ID` | Google Cloud project for Vertex AI | *(required)* |
//! | `VERTEX_AI_LOCATION` | Vertex AI location | *(required)* |
//! | `GOOGLE_APPLICATION_CREDENTIALS` | Optional local Application Default Credentials file | unset |
//! | `VERTEX_AI_MODEL` | Schema generation/repair model | `gemini-3.1-pro-preview` |
//! | `CRAWLER_VERTEX_AI_CHEAP_MODEL` | Default low-risk crawler LLM model | `gemini-3.1-flash-lite` |
//! | `CRAWLER_VERTEX_AI_STATE_MAPPING_MODEL` | Optional state mapping model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL` | Optional URL classification model override | `CRAWLER_VERTEX_AI_CHEAP_MODEL` |
//! | `CRAWLER_LLM_MAX_CONCURRENT_REQUESTS` | Max in-flight crawler LLM calls | `1` |
//! | `CRAWLER_LLM_MIN_REQUEST_INTERVAL_MS` | Minimum delay between LLM request starts | `2000` |
//! | `LOCAL_DB_URL`   | Hardcoded local DB URL                | `postgres://postgres:postgres@localhost:5432/crawler_demo` |
//! | `CRAWLER_REVIEW_REQUIRED` | Block generated patterns/schemas until approved | unset / `false`                       |
//! | `CRAWLER_REVIEW_URL_PATTERN_REQUIRED` | Block generated URL patterns until approved | unset / `false`            |
//! | `CRAWLER_REVIEW_BIND_ADDR` | Review UI bind address        | `127.0.0.1:7878`                                |
//! | `CRAWLER_REVIEW_AUTH_TOKEN` | Optional bearer token for the review UI/API | unset                               |
//! | `LOG_LEVEL`      | Global log level                     | `info`                                           |
//! | `CRAWLER_LOG_LEVEL` | Crawler-internal log level        | `info`                                           |
//!
//! Scraped products are written to `scraped_products.json` instead of calling the Product upsert use case.

use shop_core::shop_id::ShopId;
use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use crawler::llm_runtime::{CrawlerLlmGovernor, CrawlerLlmRateLimitConfig};
use crawler::local_db::{DEMO_DB_NAME, bootstrap_local_database, demo_db_url};
use crawler::logging::HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE;
use crawler::review::repository::CrawlerReviewRepository;
use crawler::review::server::{ReviewServer, ReviewServerConfig};
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::css_selector::removed_page_schema_repository::RemovedPageSchemaRepositoryImpl;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_SCHEMA_SEED_PAGES, ReqwestHtmlFetcher, ScraperServiceImpl,
};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::product_push::FileProductPushService;
use crawler::service::shop_registration::{
    RegisteredShop, ShopRegistrationRepositoryImpl, ShopRegistrationService,
    ShopRegistrationSource, ShopSyncError,
};
use crawler::spider::advisory_lock::LocalLockManager;
use crawler::spider::candidate_service::SpiderCandidateServiceImpl;
use crawler::spider::classification::url_classification_service::UrlClassificationServiceImpl;
use crawler::spider::classification::url_metadata_repository::UrlMetadataRepositoryImpl;
use crawler::spider::classification::url_pattern_repository::ShopUrlPatternRepositoryImpl;
use crawler::spider::classification::url_pattern_service::UrlPatternServiceImpl;
use crawler::spider::discovery::website_spider::SpiderImpl;
use crawler::spider::service::spider_service::{SpiderServiceConfig, SpiderServiceImpl};
use crawler::vertex_ai::{CrawlerVertexAiConfig, CrawlerVertexAiModels};
use shop_core::domain::Domain;
use shop_core::shop_type::ShopType;
use tracing::{Instrument, error, info};

// ---------------------------------------------------------------------------
// Demo shop source — returns hardcoded shops (no OpenSearch needed)
// ---------------------------------------------------------------------------

struct DemoShopSource {
    shops: Vec<RegisteredShop>,
}

#[async_trait]
impl ShopRegistrationSource for DemoShopSource {
    async fn fetch_registered_shops(&self) -> Result<Vec<RegisteredShop>, ShopSyncError> {
        Ok(self.shops.clone())
    }
}

fn crawler_review_required() -> bool {
    env::var("CRAWLER_REVIEW_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn crawler_review_url_pattern_required() -> bool {
    env::var("CRAWLER_REVIEW_URL_PATTERN_REQUIRED")
        .map(|value| matches!(value.as_str(), "true" | "TRUE" | "1" | "yes" | "YES"))
        .unwrap_or(false)
}

fn demo_shops() -> Vec<RegisteredShop> {
    // UUIDs are stable across runs so the upsert-on-conflict keeps the same rows
    // rather than creating a new shop row every time the demo starts.
    // These demo domains intentionally focus on antique marketplaces and
    // independent commercial dealers.
    [
        (
            1,
            "Hingstons Antiques",
            "hingstons-antiques",
            "www.hingstons-antiques.co.uk",
            ShopType::CommercialDealer,
        ),
        (
            2,
            "Harrison Antique Furniture",
            "harrison-antique-furniture",
            "www.harrisonantiquefurniture.co.uk",
            ShopType::CommercialDealer,
        ),
        (
            3,
            "Collinge Antiques",
            "collinge-antiques",
            "www.collingeantiques.com",
            ShopType::CommercialDealer,
        ),
        (
            4,
            "Jonathan Horne Antiques",
            "jonathan-horne-antiques",
            "www.jonathanhorne.com",
            ShopType::CommercialDealer,
        ),
        (
            5,
            "William Cook Antiques",
            "william-cook-antiques",
            "www.williamcookantiques.com",
            ShopType::CommercialDealer,
        ),
        (
            6,
            "Georgian Antiques",
            "georgian-antiques",
            "www.georgianantiques.net",
            ShopType::CommercialDealer,
        ),
        (
            7,
            "Antik & Stil",
            "antik-und-stil",
            "antik-und-stil.com",
            ShopType::CommercialDealer,
        ),
        (
            8,
            "Smeerling Antiques",
            "smeerling-antiques",
            "www.smeerling-antiques.com",
            ShopType::CommercialDealer,
        ),
        (
            9,
            "Antichita San Felice",
            "antichita-san-felice",
            "internationalantiques.eu",
            ShopType::CommercialDealer,
        ),
        (10, "Antiga", "antiga", "antiga.es", ShopType::Marketplace),
        (
            11,
            "Galerie Vauclair",
            "galerie-vauclair",
            "galerie-vauclair.fr",
            ShopType::CommercialDealer,
        ),
    ]
    .into_iter()
    .map(|(index, shop_name, shop_slug, domain, shop_type)| {
        demo_shop(index, shop_name, shop_slug, domain, shop_type)
    })
    .collect()
}

fn demo_shop(
    index: u8,
    shop_name: &str,
    shop_slug: &str,
    domain: &str,
    shop_type: ShopType,
) -> RegisteredShop {
    RegisteredShop {
        shop_id: ShopId::try_from(format!("a1000000-0000-0000-0000-{index:012}")).unwrap(),
        shop_name: shop_name.to_string(),
        shop_slug: shop_slug.to_string(),
        shop_type,
        domains: HashSet::from([Domain::try_from(domain).unwrap()]),
    }
}

// ---------------------------------------------------------------------------
// CLI flag parsing
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    init_logging();

    async {
        let vertex_ai_config = match CrawlerVertexAiConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                error!(%error, "Failed to load Vertex AI configuration");
                return;
            }
        };
        let vertex_ai_models = CrawlerVertexAiModels::from_env();

        let config = CrawlerCronConfig {
            spider_interval: Duration::from_secs(120),
            scraper_interval: Duration::from_secs(30),
            scraper_urls_per_domain: 50,
            spider_concurrency: 100,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 10,
            spider_classify_threshold: 400,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            ..Default::default()
        };

        let db_url = demo_db_url();
        if let Err(error) = bootstrap_local_database(DEMO_DB_NAME).await {
            error!(error = ?error, "Failed to bootstrap local Postgres database");
            return;
        }

        info!("Waiting for Postgres to be ready…");
        let pool = match connect_with_retry(&config, &db_url).await {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "Failed to connect to Postgres after retries");
                return;
            }
        };

        if let Err(error) = sqlx::migrate!("./migrations").run(&pool).await {
            error!(error = ?error, "Failed to apply database migrations");
            return;
        }
        info!("Database migrations applied successfully");

        let review_required = crawler_review_required();
        let url_pattern_review_required = crawler_review_url_pattern_required();
        let review_config =
            ReviewServerConfig::from_env().expect("CRAWLER_REVIEW_BIND_ADDR must be host:port");
        let review_repo = CrawlerReviewRepository::new(pool.clone());

        info!(
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            state_mapping_model = %vertex_ai_models.product_state_mapping,
            url_classification_model = %vertex_ai_models.url_classification,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Wiring crawler dependencies..."
        );
        let llm_governor = Arc::new(CrawlerLlmGovernor::new(
            CrawlerLlmRateLimitConfig::from_env(),
        ));

        let state_llm =
            match vertex_ai_config.create_model(vertex_ai_models.product_state_mapping.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for state mapping");
                    return;
                }
            };
        let state_mapping_repo = Box::new(ProductStateMappingRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));
        let state_mapping_svc = ProductStateMappingServiceImpl::new(
            state_llm,
            state_mapping_repo,
            Some(Arc::clone(&llm_governor)),
        );
        let normalization_svc = ProductNormalizationServiceImpl::new(Box::new(state_mapping_svc));

        let create_schema_llm =
            match vertex_ai_config.create_model(vertex_ai_models.product_schema.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for schema generation");
                    return;
                }
            };
        let single_schema_llm = match vertex_ai_config
            .create_model(vertex_ai_models.product_schema.clone())
        {
            Ok(model) => model,
            Err(error) => {
                error!(%error, "Failed to initialize Vertex AI model for fresh schema generation");
                return;
            }
        };

        let schema_repo = Box::new(ShopsProductSchemaRepositoryImpl::new(Box::leak(Box::new(
            pool.clone(),
        ))));
        let schema_svc = ProductSchemaServiceImpl::new(
            create_schema_llm,
            single_schema_llm,
            schema_repo,
            Some(Arc::clone(&llm_governor)),
        );
        let removed_page_schema_repo = Box::new(RemovedPageSchemaRepositoryImpl::new(Box::leak(
            Box::new(pool.clone()),
        )));

        let scraper_candidates = Box::new(
            ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
                pool.clone(),
                config.scraper_max_llm_calls_per_shop,
            ),
        );

        let fetcher = Box::new(ReqwestHtmlFetcher::with_auto_throttle_config(
            config.scraper_auto_throttle_config(),
        ));
        let scraper_svc = Box::new(
            ScraperServiceImpl::new_with_schema_seed_pages(
                fetcher,
                Box::new(schema_svc),
                Box::new(normalization_svc),
                Arc::new(
                    ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
                        pool.clone(),
                        config.scraper_max_llm_calls_per_shop,
                    ),
                ),
                config.scraper_schema_seed_pages,
                config.scraper_max_llm_calls_per_shop,
            )
            .with_removed_page_schema_repository(removed_page_schema_repo)
            .with_review_gate(review_repo.clone(), review_required),
        );

        let url_metadata_repo = Arc::new(UrlMetadataRepositoryImpl::new(pool.clone()));
        let url_pattern_repo = Box::new(ShopUrlPatternRepositoryImpl::new(pool.clone()));

        let classification_llm =
            match vertex_ai_config.create_model(vertex_ai_models.url_classification.clone()) {
                Ok(model) => model,
                Err(error) => {
                    error!(%error, "Failed to initialize Vertex AI model for URL classification");
                    return;
                }
            };
        let class_svc = Box::new(UrlClassificationServiceImpl::new(
            classification_llm,
            Some(Arc::clone(&llm_governor)),
        ));
        let pattern_svc = Box::new(UrlPatternServiceImpl::new_with_review(
            Arc::new(*url_pattern_repo),
            class_svc,
            review_repo.clone(),
            url_pattern_review_required,
        ));

        let spider_svc = Box::new(SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size: 40,
                ..Default::default()
            },
            Box::new(SpiderImpl::new(config.spider_website_config())),
            pattern_svc,
            url_metadata_repo.clone(),
        ));
        let spider_candidates = Box::new(SpiderCandidateServiceImpl::new(pool.clone()));

        let shop_source = Box::new(DemoShopSource {
            shops: demo_shops(),
        });
        let shop_repo = Box::new(ShopRegistrationRepositoryImpl::new(pool.clone()));
        let shop_registration = ShopRegistrationService::new(shop_source, shop_repo);
        let product_push = Box::new(FileProductPushService::new("scraped_products.json"));

        let cron_job = CrawlerCronJob::new(
            config,
            Arc::new(LocalLockManager::new()),
            spider_candidates,
            spider_svc,
            scraper_candidates,
            scraper_svc,
            shop_registration,
            product_push,
        );

        info!(
            shop_count = demo_shops().len(),
            llm_provider = "vertex_ai",
            schema_model = %vertex_ai_models.product_schema,
            state_mapping_model = %vertex_ai_models.product_state_mapping,
            url_classification_model = %vertex_ai_models.url_classification,
            review_required,
            url_pattern_review_required,
            review_bind_addr = %review_config.bind_addr,
            "Crawler demo is fully initialized. Starting background tasks. Press Ctrl+C to stop."
        );
        let review_server = ReviewServer::new(review_repo, review_config);
        let review_handle = tokio::spawn(async move {
            review_server
                .run()
                .await
                .expect("crawler review server failed")
        });
        let cron_handle = tokio::spawn(async move {
            cron_job.run_loop().await;
        });

        tokio::select! {
            result = review_handle => {
                result.expect("crawler review server task panicked");
            }
            result = cron_handle => {
                result.expect("crawler cron task panicked");
            }
        }
    }
    .instrument(tracing::info_span!(
        "crawler_demo",
        entrypoint = "demo",
        database = DEMO_DB_NAME
    ))
    .await;
}

// ---------------------------------------------------------------------------
// Database helpers
// ---------------------------------------------------------------------------

/// Runs `docker compose up -d` from the crawler crate directory.
///
/// `docker compose up -d` is idempotent:
/// - Container already running → no-op, returns immediately.
/// - Container exists but is stopped → restarts it.
/// - Container does not exist → creates and starts it.
///
/// The compose file path is baked in via `CARGO_MANIFEST_DIR` so this works
/// regardless of the working directory when `cargo run` is invoked.
/// Attempts to connect to Postgres, retrying with exponential back-off.
/// This handles the window between `docker compose up -d` returning and
/// Postgres actually accepting connections.
#[tracing::instrument(skip(config), fields(db_url = %db_url))]
async fn connect_with_retry(
    config: &CrawlerCronConfig,
    db_url: &str,
) -> Result<sqlx::PgPool, String> {
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(200);

    loop {
        attempt += 1;
        match config.connect_pool(db_url).await {
            Ok(pool) => {
                info!(
                    attempt,
                    max_connections = config.effective_db_max_connections(),
                    "Connected to Postgres"
                );
                return Ok(pool);
            }
            Err(e) if attempt < 30 => {
                info!(attempt, error = %e, "Postgres not ready yet, retrying…");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(3));
            }
            Err(e) => {
                return Err(format!(
                    "Could not connect to Postgres after {attempt} attempts: {e}"
                ));
            }
        }
    }
}

fn init_logging() {
    let raw_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let crawler_level = env::var("CRAWLER_LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = tracing_subscriber::EnvFilter::new(format!(
        "{raw_level},crawler={crawler_level},spider=warn,sqlx::postgres::notice=warn,{HTML5EVER_TREE_BUILDER_LOG_DIRECTIVE}"
    ));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}
