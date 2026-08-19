use crate::scraper::scraper_service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, DEFAULT_SCHEMA_SEED_PAGES, ScraperAutoThrottleConfig,
};
use crate::spider::discovery::website_spider::CrawlerConfig as SpiderCrawlerConfig;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[derive(Clone)]
pub struct CrawlerCronConfig {
    pub spider_interval: Duration,
    pub scraper_interval: Duration,
    pub shop_sync_interval: Duration,
    /// Optional number of domains fetched per scraper scheduler refill.
    /// Defaults to scraper concurrency.
    pub scraper_domain_batch_size: Option<usize>,
    /// Maximum URLs fetched per selected scraper domain.
    pub scraper_urls_per_domain: i64,
    /// Number of scraped products to accumulate before flush.
    pub push_batch_size: usize,
    pub spider_concurrency: usize,
    /// Per-site in-flight crawl limit for spider::Website.
    pub spider_site_concurrency_limit: usize,
    pub scraper_concurrency: usize,
    pub spider_classify_threshold: usize,
    /// Number of pages used to seed first-time schema generation per shop.
    /// `1` means current page only; higher values fetch additional random
    /// product pages on schema cache miss.
    pub scraper_schema_seed_pages: usize,
    /// Minimum delay before scraper requests for the same domain.
    pub scraper_domain_delay: Duration,
    /// Desired average in-flight scraper requests per domain for adaptive pacing.
    pub scraper_auto_throttle_target_concurrency: f64,
    /// Maximum adaptive delay before scraper requests for a slow domain.
    pub scraper_auto_throttle_max_delay: Duration,
    /// Smoothing factor for per-domain scraper fetch latency.
    pub scraper_auto_throttle_alpha: f64,
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
            scraper_domain_batch_size: None,
            scraper_urls_per_domain: 100,
            push_batch_size: 25,
            spider_concurrency: 3,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 10,
            spider_classify_threshold: 200,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            scraper_domain_delay: Duration::from_secs(2),
            scraper_auto_throttle_target_concurrency: 2.0,
            scraper_auto_throttle_max_delay: Duration::from_secs(10),
            scraper_auto_throttle_alpha: 0.15,
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

    pub fn spider_website_config(&self) -> SpiderCrawlerConfig {
        SpiderCrawlerConfig {
            concurrency_limit: self.spider_site_concurrency_limit,
            ..SpiderCrawlerConfig::default()
        }
    }

    pub fn scraper_auto_throttle_config(&self) -> ScraperAutoThrottleConfig {
        ScraperAutoThrottleConfig {
            target_concurrency: self.scraper_auto_throttle_target_concurrency,
            min_delay: self.scraper_domain_delay,
            max_delay: self.scraper_auto_throttle_max_delay,
            alpha: self.scraper_auto_throttle_alpha,
            enabled: true,
        }
    }

    pub fn effective_scraper_domain_batch_size(&self) -> usize {
        self.scraper_domain_batch_size
            .unwrap_or(self.scraper_concurrency)
            .max(1)
    }

    pub async fn connect_pool(&self, url: &str) -> Result<PgPool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(self.effective_db_max_connections())
            .acquire_timeout(Duration::from_secs(30))
            .connect(url)
            .await
    }
}
