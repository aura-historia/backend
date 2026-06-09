use crate::scraper::scraper_service::{DEFAULT_MAX_LLM_CALLS_PER_SHOP, DEFAULT_SCHEMA_SEED_PAGES};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[derive(Clone)]
pub struct CrawlerCronConfig {
    pub spider_interval: Duration,
    pub scraper_interval: Duration,
    pub shop_sync_interval: Duration,
    pub spider_batch_size: i64,
    pub scraper_batch_size: i64,
    /// Number of scraped products to accumulate before flush.
    pub push_batch_size: usize,
    pub spider_concurrency: usize,
    pub scraper_concurrency: usize,
    pub spider_classify_threshold: usize,
    /// Number of pages used to seed first-time schema generation per shop.
    /// `1` means current page only; higher values fetch additional random
    /// product pages on schema cache miss.
    pub scraper_schema_seed_pages: usize,
    /// Delay between consecutive scraper requests for the same domain.
    pub scraper_domain_delay: Duration,
    /// Hard per-shop budget for schema-generation LLM calls.
    pub scraper_max_llm_calls_per_shop: i64,
    /// Maximum Postgres connections for crawler queries.
    pub db_max_connections: Option<u32>,
}

impl Default for CrawlerCronConfig {
    fn default() -> Self {
        Self {
            spider_interval: Duration::from_secs(600), // 10 minutes
            scraper_interval: Duration::from_secs(60), // 1 minute
            shop_sync_interval: Duration::from_secs(10800), // 3 hours
            spider_batch_size: 10,
            scraper_batch_size: 100,
            push_batch_size: 25,
            spider_concurrency: 3,
            scraper_concurrency: 10,
            spider_classify_threshold: 200,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            scraper_domain_delay: Duration::from_secs(1),
            scraper_max_llm_calls_per_shop: DEFAULT_MAX_LLM_CALLS_PER_SHOP,
            db_max_connections: None,
        }
    }
}

impl CrawlerCronConfig {
    pub fn effective_db_max_connections(&self) -> u32 {
        self.db_max_connections
            .unwrap_or_else(|| (self.spider_concurrency + self.scraper_concurrency + 10) as u32)
    }

    pub async fn connect_pool(&self, url: &str) -> Result<PgPool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(self.effective_db_max_connections())
            .acquire_timeout(Duration::from_secs(30))
            .connect(url)
            .await
    }
}
