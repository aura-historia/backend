use async_trait::async_trait;
use common::pagination::cursor::Cursor;
use common::shop_id::ShopId;
use crawler::scraper::candidate_service::ScraperCandidateServiceImpl;
use crawler::scraper::css_selector::product_schema_repository::ShopsProductSchemaRepositoryImpl;
use crawler::scraper::css_selector::product_schema_service::ProductSchemaServiceImpl;
use crawler::scraper::normalization::product_normalization_service::ProductNormalizationServiceImpl;
use crawler::scraper::normalization::state_mapping_repository::ProductStateMappingRepositoryImpl;
use crawler::scraper::normalization::state_mapping_service::ProductStateMappingServiceImpl;
use crawler::scraper::scraper_service::{ScraperServiceImpl, SpiderHtmlFetcher};
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::shop_registration::{
    RegisteredShop, ShopRegistrationRepositoryImpl, ShopRegistrationService,
    ShopRegistrationSource, ShopSyncError,
};
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
use shop::core::shop_search::ShopSearch;
use shop::opensearch::repository::ShopOpenSearchRepositoryImpl;
use shop::service::query_service::{QueryShopService, QueryShopServiceImpl};
use sqlx::PgPool;
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

                all_shops.push(RegisteredShop {
                    shop_id,
                    shop_name: name,
                    shop_slug: slug,
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

    common::logging::init_logging();

    info!("Starting Crawler Server");

    // 1. Connect to database
    let db_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set");
    let pool = PgPool::connect(&db_url)
        .await
        .expect("Failed to connect to database");

    // 2. Wire dependencies
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

    // 3. Wire shop registration (sync from OpenSearch)
    let opensearch_client = build_opensearch_client();
    let shop_source = Box::new(OpenSearchShopSource { opensearch_client });
    let shop_repo = Box::new(ShopRegistrationRepositoryImpl::new(pool.clone()));
    let shop_registration = ShopRegistrationService::new(shop_source, shop_repo);

    // 4. Build Cron Job
    let config = CrawlerCronConfig {
        spider_interval: Duration::from_secs(600),
        scraper_interval: Duration::from_secs(60),
        spider_batch_size: 10,
        scraper_batch_size: 20,
        spider_concurrency: 3,
        scraper_concurrency: 10,
        spider_classify_threshold: 200,
        ..Default::default()
    };

    let cron_job = CrawlerCronJob::new(
        config,
        spider_candidates,
        spider_svc,
        scraper_candidates,
        scraper_svc,
        shop_registration,
    );

    // 5. Run forever
    info!("Crawler Server is fully initialized. Starting background tasks...");
    cron_job.run_loop().await;
}
