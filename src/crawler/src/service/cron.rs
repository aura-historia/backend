use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::product_push::{ProductPushService, normalize_to_upsert};
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::advisory_lock::{DomainAdvisoryLock, UrlAdvisoryLock};
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use futures::StreamExt;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct CrawlerCronConfig {
    pub spider_interval: Duration,
    pub scraper_interval: Duration,
    pub shop_sync_interval: Duration,
    pub spider_batch_size: i64,
    pub scraper_batch_size: i64,
    /// Number of scraped products to accumulate before flushing a push to the backend.
    /// Keeps memory bounded and avoids holding all results until the last scrape finishes.
    pub push_batch_size: usize,
    pub spider_concurrency: usize,
    pub scraper_concurrency: usize,
    pub spider_classify_threshold: usize,
    /// Maximum number of Postgres connections in the pool.
    ///
    /// Each concurrent spider task holds one connection for its `DomainAdvisoryLock`
    /// for the full duration of the crawl, and each concurrent scraper task holds one
    /// for its `UrlAdvisoryLock`.  On top of those long-lived locks there are short-lived
    /// query connections issued by repository impls and the shop-sync task.
    ///
    /// When `None` the value is computed automatically as:
    ///   `spider_concurrency + scraper_concurrency + 10`
    /// which provides comfortable headroom for queries and advisory-lock releases
    /// without wasting idle connections.
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
            db_max_connections: None,
        }
    }
}

impl CrawlerCronConfig {
    /// Returns the effective `max_connections` for the Postgres pool.
    ///
    /// Uses the explicit override when set; otherwise auto-computes from the
    /// concurrency settings.  The auto value is
    /// `spider_concurrency + scraper_concurrency + 10`.
    pub fn effective_db_max_connections(&self) -> u32 {
        self.db_max_connections
            .unwrap_or_else(|| (self.spider_concurrency + self.scraper_concurrency + 10) as u32)
    }

    /// Builds a [`PgPool`] whose size is appropriate for this config.
    ///
    /// Prefer this over calling `PgPool::connect` directly so that the pool is
    /// always large enough to serve all concurrent advisory-lock connections plus
    /// the repository queries issued within each task.
    pub async fn connect_pool(&self, url: &str) -> Result<PgPool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(self.effective_db_max_connections())
            .acquire_timeout(Duration::from_secs(30))
            .connect(url)
            .await
    }
}

#[derive(Clone)]
pub struct CrawlerCronJob {
    config: CrawlerCronConfig,
    /// Pool used to acquire per-domain advisory locks before spidering.
    /// When `None` (e.g. in unit tests), locking is skipped and every candidate
    /// is run unconditionally.
    pool: Option<PgPool>,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    scraper_service: Arc<dyn ScraperService>,
    shop_registration: Arc<ShopRegistrationService>,
    product_push: Arc<dyn ProductPushService>,
    /// Rolling counters for periodic scraper performance summaries (reset every 500 URLs).
    scraper_perf_urls: Arc<AtomicU64>,
    scraper_perf_duration_ms: Arc<AtomicU64>,
    /// Rolling counters for periodic spider performance summaries (reset every 50 domains).
    spider_perf_domains: Arc<AtomicU64>,
    spider_perf_duration_ms: Arc<AtomicU64>,
}

impl CrawlerCronJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CrawlerCronConfig,
        pool: PgPool,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
        shop_registration: ShopRegistrationService,
        product_push: Box<dyn ProductPushService>,
    ) -> Self {
        Self {
            config,
            pool: Some(pool),
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            shop_registration: Arc::new(shop_registration),
            product_push: product_push.into(),
            scraper_perf_urls: Arc::new(AtomicU64::new(0)),
            scraper_perf_duration_ms: Arc::new(AtomicU64::new(0)),
            spider_perf_domains: Arc::new(AtomicU64::new(0)),
            spider_perf_duration_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Creates a `CrawlerCronJob` without a database pool.
    ///
    /// Advisory lock acquisition is skipped — every spider candidate runs
    /// unconditionally. Intended for unit tests and environments where the DB
    /// pool is not available at construction time.
    #[cfg(test)]
    fn new_without_pool(
        config: CrawlerCronConfig,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
        shop_registration: ShopRegistrationService,
        product_push: Box<dyn ProductPushService>,
    ) -> Self {
        Self {
            config,
            pool: None,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            shop_registration: Arc::new(shop_registration),
            product_push: product_push.into(),
            scraper_perf_urls: Arc::new(AtomicU64::new(0)),
            scraper_perf_duration_ms: Arc::new(AtomicU64::new(0)),
            spider_perf_domains: Arc::new(AtomicU64::new(0)),
            spider_perf_duration_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");

        let spider_job = self.clone();
        let sync_job = self.clone();
        let scraper_job = self;

        let spider_handle = tokio::spawn(async move {
            spider_job.spider_loop().await;
        });

        let scraper_handle = tokio::spawn(async move {
            scraper_job.scraper_loop().await;
        });

        let sync_handle = tokio::spawn(async move {
            sync_job.shop_sync_loop().await;
        });

        let _ = tokio::join!(spider_handle, scraper_handle, sync_handle);
    }

    async fn spider_loop(&self) {
        loop {
            self.run_spider_once().await;
            tokio::time::sleep(self.config.spider_interval).await;
        }
    }

    async fn scraper_loop(&self) {
        loop {
            self.run_scraper_once().await;
            tokio::time::sleep(self.config.scraper_interval).await;
        }
    }

    async fn shop_sync_loop(&self) {
        // Run immediately on startup, then every shop_sync_interval
        loop {
            self.run_shop_sync_once().await;
            tokio::time::sleep(self.config.shop_sync_interval).await;
        }
    }

    async fn run_shop_sync_once(&self) {
        match self.shop_registration.sync().await {
            Ok(count) => {
                if count > 0 {
                    info!(count, "Shop sync tick complete");
                }
            }
            Err(e) => error!(error = %e, "Shop sync failed"),
        }
    }

    async fn run_spider_once(&self) {
        match self
            .spider_candidates
            .get_candidates(self.config.spider_batch_size)
            .await
        {
            Ok(candidates) => {
                if candidates.is_empty() {
                    debug!("No spider candidates, skipping batch");
                    return;
                }
                let total = candidates.len();
                let batch_start = tokio::time::Instant::now();
                info!(candidates = total, "Spider batch starting");

                let results: Vec<bool> = futures::stream::iter(candidates)
                    .map(|candidate| {
                        let spider_service = self.spider_service.clone();
                        let pool = self.pool.clone();
                        let threshold = self.config.spider_classify_threshold;
                        let shop_url = if candidate.shop_domain.starts_with("http") {
                            candidate.shop_domain.clone()
                        } else {
                            format!("https://{}", candidate.shop_domain)
                        };
                        async move {
                            // Acquire an advisory lock when a pool is available.
                            // In test contexts where no pool is provided, locking is
                            // skipped and the spider runs unconditionally.
                            let _lock = if let Some(ref p) = pool {
                                match DomainAdvisoryLock::try_acquire(p, candidate.domain_id).await
                                {
                                    Ok(Some(lock)) => Some(lock),
                                    Ok(None) => {
                                        warn!(
                                            domain_id = %candidate.domain_id,
                                            "Skipping domain — advisory lock held by another worker"
                                        );
                                        return false;
                                    }
                                    Err(e) => {
                                        error!(
                                            domain_id = %candidate.domain_id,
                                            error = %e,
                                            "Failed to acquire advisory lock"
                                        );
                                        return false;
                                    }
                                }
                            } else {
                                None
                            };

                            let success = spider_service
                                .run(&candidate.shop_id, &candidate.domain_id, &shop_url, threshold)
                                .await
                                .map_err(|e| {
                                    error!(domain = %candidate.shop_domain, error = %e, "Spider run failed");
                                })
                                .is_ok();
                            // `_lock` dropped here → pg_advisory_unlock called automatically
                            success
                        }
                    })
                    .buffer_unordered(self.config.spider_concurrency)
                    .collect()
                    .await;

                let succeeded = results.iter().filter(|&&ok| ok).count();
                let failed = total - succeeded;
                let duration_ms = batch_start.elapsed().as_millis() as u64;
                info!(
                    total,
                    succeeded, failed, duration_ms, "Spider batch complete"
                );

                // Accumulate perf counters; emit summary every 50 domains.
                self.spider_perf_domains
                    .fetch_add(total as u64, Ordering::Relaxed);
                self.spider_perf_duration_ms
                    .fetch_add(duration_ms, Ordering::Relaxed);
                let perf_domains = self.spider_perf_domains.load(Ordering::Relaxed);
                if perf_domains >= 50 {
                    let perf_ms = self.spider_perf_duration_ms.load(Ordering::Relaxed);
                    let avg_ms = perf_ms / perf_domains;
                    info!(
                        domains_processed = perf_domains,
                        avg_spider_ms = avg_ms,
                        "Spider performance summary"
                    );
                    self.spider_perf_domains.store(0, Ordering::Relaxed);
                    self.spider_perf_duration_ms.store(0, Ordering::Relaxed);
                }
            }
            Err(e) => error!(error = %e, "Failed to retrieve spider candidates"),
        }
    }

    async fn run_scraper_once(&self) {
        match self
            .scraper_candidates
            .get_candidates(self.config.scraper_batch_size)
            .await
        {
            Ok(candidates) => {
                if candidates.is_empty() {
                    debug!("No scraper candidates, skipping batch");
                    return;
                }
                let total = candidates.len();
                let batch_start = tokio::time::Instant::now();
                info!(candidates = total, "Scraper batch starting");

                // Stream scrape results and push them in chunks as they arrive,
                // rather than collecting everything into memory first.
                let push_service = self.product_push.clone();
                let push_batch_size = self.config.push_batch_size;

                // Collect (Option<cmd>, errored) tuples so we can count
                // succeeded/failed after the stream completes.
                let results: Vec<(Option<_>, bool)> = futures::stream::iter(candidates)
                    .map(|candidate| {
                        let scraper_service = self.scraper_service.clone();
                        let pool = self.pool.clone();
                        #[allow(clippy::let_and_return)]
                        async move {
                            // Acquire a per-URL advisory lock so that two concurrent
                            // workers never scrape the same URL at the same time.
                            // When no pool is available (unit tests), locking is skipped.
                            let _lock = if let Some(ref p) = pool {
                                match UrlAdvisoryLock::try_acquire(p, &candidate.url).await {
                                    Ok(Some(lock)) => Some(lock),
                                    Ok(None) => {
                                        warn!(
                                            url = %candidate.url,
                                            "Skipping URL — advisory lock held by another worker"
                                        );
                                        return (None, false);
                                    }
                                    Err(e) => {
                                        error!(
                                            url = %candidate.url,
                                            error = %e,
                                            "Failed to acquire advisory lock for URL"
                                        );
                                        return (None, true);
                                    }
                                }
                            } else {
                                None
                            };

                            let domain = candidate.url.host_str().unwrap_or("unknown");
                            let result = match scraper_service
                                .scrape(
                                    &candidate.shop_id,
                                    &candidate.url,
                                    &candidate.main_hash,
                                    candidate.last_scraped_hash.as_deref(),
                                )
                                .await
                            {
                                Ok(Some(normalized_product)) => {
                                    (normalize_to_upsert(normalized_product, &candidate), false)
                                }
                                Ok(None) => (None, false),
                                Err(e) => {
                                    error!(domain = %domain, url = %candidate.url, error = %e, "Scraper run failed");
                                    (None, true)
                                }
                            };
                            // `_lock` dropped here → pg_advisory_unlock called automatically
                            result
                        }
                    })
                    .buffer_unordered(self.config.scraper_concurrency)
                    .collect()
                    .await;

                let failed: usize = results.iter().filter(|(_, errored)| *errored).count();

                // Push successful products in batches
                let commands: Vec<_> = results.into_iter().filter_map(|(opt, _)| opt).collect();
                let succeeded = commands.len();

                for chunk in commands.chunks(push_batch_size) {
                    push_service.push(chunk.to_vec()).await;
                }

                let duration_ms = batch_start.elapsed().as_millis() as u64;
                let skipped = total - succeeded - failed;
                info!(
                    total,
                    succeeded, failed, skipped, duration_ms, "Scraper batch complete"
                );

                // Accumulate perf counters; emit summary every 500 URLs.
                self.scraper_perf_urls
                    .fetch_add(total as u64, Ordering::Relaxed);
                self.scraper_perf_duration_ms
                    .fetch_add(duration_ms, Ordering::Relaxed);
                let perf_urls = self.scraper_perf_urls.load(Ordering::Relaxed);
                if perf_urls >= 500 {
                    let perf_ms = self.scraper_perf_duration_ms.load(Ordering::Relaxed);
                    let avg_ms = perf_ms / perf_urls;
                    info!(
                        urls_processed = perf_urls,
                        avg_scrape_ms = avg_ms,
                        "Scraper performance summary"
                    );
                    self.scraper_perf_urls.store(0, Ordering::Relaxed);
                    self.scraper_perf_duration_ms.store(0, Ordering::Relaxed);
                }
            }
            Err(e) => error!(error = %e, "Failed to retrieve scraper candidates"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::{MockScraperCandidateService, ScraperCandidate};
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::product_push::MockProductPushService;
    use crate::service::shop_registration::{
        MockShopRegistrationRepository, MockShopRegistrationSource,
    };
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use common::shop_id::ShopId;
    use shop::core::shop_type::ShopType;

    fn noop_shop_registration() -> ShopRegistrationService {
        let mut source = MockShopRegistrationSource::new();
        source
            .expect_fetch_registered_shops()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let repository = MockShopRegistrationRepository::new();
        ShopRegistrationService::new(Box::new(source), Box::new(repository))
    }

    fn noop_product_push() -> Box<MockProductPushService> {
        let mut push = MockProductPushService::new();
        push.expect_push().returning(|_| Box::pin(async {}));
        Box::new(push)
    }

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let expected_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: expected_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service
            .expect_run()
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _| {
                Box::pin(async {
                    Ok(SpiderRunResult {
                        total_links: 10,
                        product_urls_count: 5,
                        product_pattern: None,
                    })
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new_without_pool(
            CrawlerCronConfig::default(),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            noop_product_push(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_run_scraper_candidates_and_push_products() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![ScraperCandidate {
                    shop_id: ShopId::new(),
                    shop_name: "Test Shop".to_string(),
                    shop_type: ShopType::CommercialDealer,
                    url: url::Url::parse("https://example.com/product/1").unwrap(),
                    main_hash: "hash1".to_string(),
                    last_scraped_hash: None,
                }])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        // When scraper returns None, push should NOT be called
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new_without_pool(
            CrawlerCronConfig::default(),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
            Box::new(push_service),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_run_shop_sync() {
        let mut source = MockShopRegistrationSource::new();
        source.expect_fetch_registered_shops().returning(|| {
            Box::pin(async {
                Ok(vec![crate::service::shop_registration::RegisteredShop {
                    shop_id: ShopId::new(),
                    shop_name: "Test Shop".to_string(),
                    shop_slug: "test-shop".to_string(),
                    shop_type: ShopType::CommercialDealer,
                    domains: std::collections::HashSet::from([common::domain::Domain::try_from(
                        "test.com",
                    )
                    .unwrap()]),
                }])
            })
        });

        let mut repository = MockShopRegistrationRepository::new();
        repository
            .expect_upsert_shop()
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_sync_domains()
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        repository
            .expect_deactivate_shops_not_in()
            .times(1)
            .returning(|_| Box::pin(async { Ok(0) }));

        let shop_registration =
            ShopRegistrationService::new(Box::new(source), Box::new(repository));

        let spider_candidates = MockSpiderCandidateService::new();
        let spider_service = MockSpiderService::new();
        let scraper_candidates = MockScraperCandidateService::new();
        let scraper_service = MockScraperService::new();

        let job = CrawlerCronJob::new_without_pool(
            CrawlerCronConfig::default(),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            shop_registration,
            noop_product_push(),
        );

        job.run_shop_sync_once().await;
    }
}
