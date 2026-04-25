//! Production server binary for the crawler.
//!
//! Wires all dependencies (Postgres, OpenSearch, DynamoDB, LLM) and starts the
//! [`CrawlerCronJob`] loop that continuously spiders shop websites, scrapes product pages,
//! and pushes normalized products to DynamoDB via [`CommandProductServiceImpl`].
//!
//! # Connection pool sizing
//!
//! `db_max_connections` defaults to
//! `spider_concurrency + scraper_concurrency + 10`.
//! Override it explicitly in [`CrawlerCronConfig`] if needed.
//!
//! # Required environment variables
//!
//! | Variable                  | Purpose                                                        |
//! |---------------------------|----------------------------------------------------------------|
//! | `DATABASE_URL`            | Postgres connection string                                     |
//! | `GEMINI_API_KEY`          | API key for the Gemini LLM backend                             |
//! | `GEMINI_MODEL`            | Gemini model name (default: `gemini-3.1-pro-preview`)   |
//! | `DYNAMODB_TABLE_NAME`     | DynamoDB table for product events                              |
//! | `OPENSEARCH_ENDPOINT_URL` | OpenSearch base URL                                            |
//! | `OPENSEARCH_USERNAME`     | OpenSearch username                                            |
//! | `OPENSEARCH_PASSWORD`     | OpenSearch password                                            |

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use common::pagination::cursor::Cursor;
use common::price::domain::FixedFxRate;
use common::shop_id::ShopId;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{
    DEFAULT_SCHEMA_SEED_PAGES, ReqwestHtmlFetcher, ScraperServiceImpl,
};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::product_push::ProductPushServiceImpl;
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
use llm::builder::{LLMBackend, LLMBuilder};
use opensearch::auth::Credentials;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use product::service::command_service::CommandProductServiceImpl;
use product_classification::category::dynamodb_repository::CategoryDynamoDbRepositoryImpl;
use product_classification::category::opensearch_repository::CategoryOpenSearchRepositoryImpl;
use product_classification::category::service::CategoryServiceImpl;
use product_classification::period::dynamodb_repository::PeriodDynamoDbRepositoryImpl;
use product_classification::period::opensearch_repository::PeriodOpenSearchRepositoryImpl;
use product_classification::period::service::PeriodServiceImpl;
use shop::core::shop_search::ShopSearch;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::query_service::{QueryShopService, QueryShopServiceImpl};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

// ---------------------------------------------------------------------------
// ShopRegistrationSource backed by QueryShopService (OpenSearch)
// ---------------------------------------------------------------------------

struct OpenSearchShopSource {
    opensearch_client: opensearch::OpenSearch,
}

#[async_trait]
impl ShopRegistrationSource for OpenSearchShopSource {
    async fn fetch_registered_shops(&self) -> Result<Vec<RegisteredShop>, ShopSyncError> {
        let repository = ShopOpenSearchRepositoryImpl::new(&self.opensearch_client);
        let query_service = QueryShopServiceImpl::new(&repository);

        let search = ShopSearch::default();
        let mut all_shops = Vec::new();
        let mut cursor: Option<Cursor<serde_json::Value>> = None;

        loop {
            let result = query_service
                .search_shops(&search, &None, &cursor)
                .await
                .map_err(|e| ShopSyncError::FetchError(e.to_string()))?;

            let page_size = result.items.len();
            for shop in result.items {
                let slug: String = shop.shop_slug_id.into();
                let name: String = shop.name.into();
                let shop_id: ShopId = shop.shop_id;
                let shop_type = shop.shop_type;

                all_shops.push(RegisteredShop {
                    shop_id,
                    shop_name: name,
                    shop_slug: slug,
                    shop_type,
                    domains: shop.domains,
                });
            }

            if page_size == 0 || result.cursor.search_after.is_none() {
                break;
            }

            cursor = Some(result.cursor);
        }

        Ok(all_shops)
    }
}

fn build_opensearch_client() -> opensearch::OpenSearch {
    let endpoint_url_str = std::env::var("OPENSEARCH_ENDPOINT_URL")
        .expect("OPENSEARCH_ENDPOINT_URL environment variable must be set");
    let endpoint_url =
        url::Url::parse(&endpoint_url_str).expect("OPENSEARCH_ENDPOINT_URL must be a valid URL");

    let username = std::env::var("OPENSEARCH_USERNAME").expect("OPENSEARCH_USERNAME must be set");
    let password = std::env::var("OPENSEARCH_PASSWORD").expect("OPENSEARCH_PASSWORD must be set");

    let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
        .auth(Credentials::Basic(username, password))
        .build()
        .expect("Failed to build OpenSearch transport");

    opensearch::OpenSearch::new(transport)
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    common::logging::init_logging_with_directives(&["spider=warn", "sqlx::postgres::notice=warn"]);

    info!("Starting Crawler Server");

    // 1. Build cron config (needed for pool sizing before everything else)
    let config = CrawlerCronConfig {
        spider_interval: Duration::from_secs(600),
        scraper_interval: Duration::from_secs(60),
        spider_batch_size: 1000,
        scraper_batch_size: 200,
        spider_concurrency: 10,
        scraper_concurrency: 10,
        spider_classify_threshold: 400,
        scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
        push_batch_size: 1000,
        ..Default::default()
    };

    // 2. Connect to database — pool is sized to spider_concurrency + scraper_concurrency + 10
    //    to keep headroom for concurrent repository queries.
    let db_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
    let pool = config
        .connect_pool(&db_url)
        .await
        .expect("Failed to connect to database");

    info!(
        max_connections = config.effective_db_max_connections(),
        "Connected to Postgres"
    );

    // 3. Apply pending migrations — runs at startup so deploying a new binary
    //    is the only step required to update the production schema.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run database migrations");
    info!("Database migrations applied successfully");

    // 4. Wire scraper + spider dependencies
    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let model =
        std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.1-pro-preview".to_string());

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

    let scraper_candidates = Box::new(
        ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
            pool.clone(),
            config.scraper_max_llm_calls_per_shop,
        ),
    );

    let fetcher = Box::new(ReqwestHtmlFetcher::new());
    let scraper_svc = Box::new(ScraperServiceImpl::new_with_schema_seed_pages(
        fetcher,
        Box::new(schema_svc),
        Box::new(normalization_svc),
        Arc::new(
            ScraperCandidateServiceImpl::new_with_max_llm_calls_per_shop(
                pool.clone(),
                config.scraper_max_llm_calls_per_shop,
            ),
        ),
        3,
        config.scraper_schema_seed_pages,
        config.scraper_max_llm_calls_per_shop,
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

    // 5. Wire shop registration (sync from OpenSearch)
    let opensearch_client = build_opensearch_client();
    let shop_source = Box::new(OpenSearchShopSource { opensearch_client });
    let shop_repo = Box::new(ShopRegistrationRepositoryImpl::new(pool.clone()));
    let shop_registration = ShopRegistrationService::new(shop_source, shop_repo);

    // 6. Wire product push — backed by DynamoDB in production
    let table_name = std::env::var("DYNAMODB_TABLE_NAME").expect("DYNAMODB_TABLE_NAME must be set");
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .load()
        .await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws_config);

    let product_dynamodb_repo = Box::leak(Box::new(ProductDynamoDbRepositoryImpl::new(
        Box::leak(Box::new(dynamodb.clone())),
        table_name.clone(),
    )));
    let fx_rate = Box::leak(Box::new(FixedFxRate()));

    let period_dynamodb_repo = Box::leak(Box::new(PeriodDynamoDbRepositoryImpl::new(
        Box::leak(Box::new(dynamodb.clone())),
        table_name.clone(),
    )));
    let category_dynamodb_repo = Box::leak(Box::new(CategoryDynamoDbRepositoryImpl::new(
        Box::leak(Box::new(dynamodb.clone())),
        table_name.clone(),
    )));

    let opensearch_for_classification = build_opensearch_client();
    let period_opensearch_repo = Box::leak(Box::new(PeriodOpenSearchRepositoryImpl::new(
        Box::leak(Box::new(opensearch_for_classification)),
    )));
    let category_opensearch_repo = Box::leak(Box::new(CategoryOpenSearchRepositoryImpl::new(
        Box::leak(Box::new(build_opensearch_client())),
    )));

    let period_svc = Box::leak(Box::new(PeriodServiceImpl::new(
        period_dynamodb_repo,
        period_opensearch_repo,
    )));
    let category_svc = Box::leak(Box::new(CategoryServiceImpl::new(
        category_dynamodb_repo,
        category_opensearch_repo,
    )));

    let command_product_service = Box::new(CommandProductServiceImpl::new(
        product_dynamodb_repo,
        fx_rate,
        period_svc,
        category_svc,
    ));
    let product_push = Box::new(ProductPushServiceImpl::new(command_product_service));

    // 7. Build cron job
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

    // 8. Run forever
    info!("Crawler Server is fully initialized. Starting background tasks...");
    cron_job.run_loop().await;
}
