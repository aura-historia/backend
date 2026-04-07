use crate::scraper::candidate_service::{ScraperCandidate, ScraperCandidateService};
use crate::scraper::scraper_service::ScraperService;
use crate::service::product_push::{ProductPushService, normalize_to_upsert};
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::advisory_lock::{DomainLock, LocalLockManager, UrlLock};
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Minimum delay between consecutive HTTP requests to the **same domain**.
    ///
    /// After each scrape completes the pipeline waits this long before
    /// dispatching the next URL for that domain.  This acts as a per-domain
    /// rate limiter and avoids triggering anti-bot protections.
    ///
    /// Set to `Duration::ZERO` to disable the delay entirely.
    pub scraper_domain_delay: Duration,
    /// Maximum number of Postgres connections in the pool.
    ///
    /// When `None` the value is computed automatically as:
    ///   `spider_concurrency + scraper_concurrency + 10`
    /// to leave headroom for short-lived query bursts.
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
            scraper_domain_delay: Duration::from_secs(1),
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
    /// always large enough for concurrent crawler queries plus short-lived
    /// repository bursts.
    pub async fn connect_pool(&self, url: &str) -> Result<PgPool, sqlx::Error> {
        PgPoolOptions::new()
            .max_connections(self.effective_db_max_connections())
            .acquire_timeout(Duration::from_secs(30))
            .connect(url)
            .await
    }
}

// ---------------------------------------------------------------------------
// PerfCounter — rolling performance summary emitted every N items
// ---------------------------------------------------------------------------

struct PerfCounter {
    count: AtomicU64,
    duration_ms: AtomicU64,
    threshold: u64,
    label: &'static str,
}

impl Clone for PerfCounter {
    fn clone(&self) -> Self {
        Self {
            count: AtomicU64::new(self.count.load(Ordering::Relaxed)),
            duration_ms: AtomicU64::new(self.duration_ms.load(Ordering::Relaxed)),
            threshold: self.threshold,
            label: self.label,
        }
    }
}

impl PerfCounter {
    fn new(threshold: u64, label: &'static str) -> Self {
        Self {
            count: AtomicU64::new(0),
            duration_ms: AtomicU64::new(0),
            threshold,
            label,
        }
    }

    /// Accumulates `count` items processed in `duration_ms` and emits an
    /// `info!` summary every time the rolling total reaches the threshold.
    fn record(&self, count: u64, duration_ms: u64) {
        self.count.fetch_add(count, Ordering::Relaxed);
        self.duration_ms.fetch_add(duration_ms, Ordering::Relaxed);

        let total = self.count.load(Ordering::Relaxed);
        if total >= self.threshold {
            let total_ms = self.duration_ms.load(Ordering::Relaxed);
            let avg_ms = total_ms / total;
            info!(
                items_processed = total,
                avg_ms = avg_ms,
                label = self.label,
                "Performance summary"
            );
            self.count.store(0, Ordering::Relaxed);
            self.duration_ms.store(0, Ordering::Relaxed);
        }
    }
}

#[derive(Clone)]
pub struct CrawlerCronJob {
    config: CrawlerCronConfig,
    lock_manager: Arc<LocalLockManager>,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    scraper_service: Arc<dyn ScraperService>,
    shop_registration: Arc<ShopRegistrationService>,
    product_push: Arc<dyn ProductPushService>,
    spider_perf: Arc<PerfCounter>,
    scraper_perf: Arc<PerfCounter>,
}

impl CrawlerCronJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: CrawlerCronConfig,
        lock_manager: Arc<LocalLockManager>,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
        shop_registration: ShopRegistrationService,
        product_push: Box<dyn ProductPushService>,
    ) -> Self {
        Self {
            config,
            lock_manager,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            shop_registration: Arc::new(shop_registration),
            product_push: product_push.into(),
            spider_perf: Arc::new(PerfCounter::new(50, "spider")),
            scraper_perf: Arc::new(PerfCounter::new(500, "scraper")),
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
                        let lock_manager = Arc::clone(&self.lock_manager);
                        let threshold = self.config.spider_classify_threshold;
                        let shop_url = if candidate.shop_domain.starts_with("http") {
                            candidate.shop_domain.clone()
                        } else {
                            format!("https://{}", candidate.shop_domain)
                        };
                        async move {
                            let Some(_lock) = DomainLock::try_acquire(&lock_manager, candidate.domain_id)
                            else {
                                warn!(
                                    domain_id = %candidate.domain_id,
                                    "Skipping domain — lock held by another worker"
                                );
                                return false;
                            };

                            spider_service
                                .run(&candidate.shop_id, &candidate.domain_id, &shop_url, threshold)
                                .await
                                .map_err(|e| {
                                    error!(domain = %candidate.shop_domain, error = %e, "Spider run failed");
                                })
                                .is_ok()
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

                self.spider_perf.record(total as u64, duration_ms);
            }
            Err(e) => error!(error = %e, "Failed to retrieve spider candidates"),
        }
    }
}

// ---------------------------------------------------------------------------
// ScrapeOutcome — result of a single scrape future in the pipeline
// ---------------------------------------------------------------------------

struct ScrapeOutcome {
    domain: String,
    command: Option<product::service::product_command::UpsertProductCommand>,
    errored: bool,
}

// ---------------------------------------------------------------------------
// scrape_candidate — free async fn executed for every slot in the pipeline
// ---------------------------------------------------------------------------

/// Acquires a per-URL lock, runs the scraper, releases the lock, waits the
/// per-domain delay, then returns a `ScrapeOutcome`.
async fn scrape_candidate(
    scraper: Arc<dyn ScraperService>,
    lock_manager: Arc<LocalLockManager>,
    candidate: ScraperCandidate,
    domain: String,
    domain_delay: Duration,
) -> ScrapeOutcome {
    let Some(_lock) = UrlLock::try_acquire(&lock_manager, &candidate.url) else {
        warn!(url = %candidate.url, "Skipping URL — lock held by another worker");
        return ScrapeOutcome {
            domain,
            command: None,
            errored: false,
        };
    };

    let outcome = match scraper
        .scrape(
            &candidate.shop_id,
            &candidate.url,
            &candidate.main_hash,
            candidate.last_scraped_hash.as_deref(),
        )
        .await
    {
        Ok(Some(product)) => ScrapeOutcome {
            command: normalize_to_upsert(product, &candidate),
            domain,
            errored: false,
        },
        Ok(None) => ScrapeOutcome {
            domain,
            command: None,
            errored: false,
        },
        Err(e) => {
            error!(
                domain = %domain,
                url = %candidate.url,
                error = %e,
                "Scraper run failed"
            );
            ScrapeOutcome {
                domain,
                command: None,
                errored: true,
            }
        }
    };
    // Per-domain delay: keep this domain's slot occupied for the configured
    // duration so the next request to the same domain is not dispatched
    // immediately.  The in-memory lock has already been released above.
    if !domain_delay.is_zero() {
        tokio::time::sleep(domain_delay).await;
    }

    outcome
}

impl CrawlerCronJob {
    async fn run_scraper_once(&self) {
        // Fetch enough candidates to keep all concurrent slots busy.  We ask
        // for scraper_concurrency × scraper_batch_size URLs so that every
        // domain gets a meaningful queue even when there are many domains.
        let total_fetch = (self.config.scraper_concurrency as i64) * self.config.scraper_batch_size;

        let all_candidates = match self.scraper_candidates.get_candidates(total_fetch).await {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "Failed to retrieve scraper candidates");
                return;
            }
        };

        if all_candidates.is_empty() {
            debug!("No scraper candidates, skipping batch");
            return;
        }

        let total = all_candidates.len();
        let batch_start = tokio::time::Instant::now();
        info!(
            candidates = total,
            concurrency = self.config.scraper_concurrency,
            "Scraper batch starting"
        );

        // Group candidates by domain so we can ensure at most one in-flight
        // request per domain at any time.
        //
        // URLs without a recognizable host are placed under the empty-string
        // key so they are still dispatched (and will fail gracefully inside
        // `scrape()` with `ScraperError::NoHost`).
        let mut by_domain: HashMap<String, VecDeque<ScraperCandidate>> = HashMap::new();
        for candidate in all_candidates {
            let domain = candidate.url.host_str().unwrap_or("").to_string();
            by_domain.entry(domain).or_default().push_back(candidate);
        }

        debug!(domains = by_domain.len(), "Candidates grouped by domain");

        let concurrency = self.config.scraper_concurrency;
        let domain_delay = self.config.scraper_domain_delay;
        let mut in_flight: HashSet<String> = HashSet::new();
        let mut futures: FuturesUnordered<
            std::pin::Pin<Box<dyn std::future::Future<Output = ScrapeOutcome> + Send>>,
        > = FuturesUnordered::new();

        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut pending_commands: Vec<product::service::product_command::UpsertProductCommand> =
            Vec::new();
        let push_service = self.product_push.clone();
        let push_batch_size = self.config.push_batch_size;

        // Fill open concurrency slots from domains that are not currently
        // in-flight.  Runs once to prime the pipeline and again after each
        // future completes.
        let fill_slots = |by_domain: &mut HashMap<
            String,
            VecDeque<crate::scraper::candidate_service::ScraperCandidate>,
        >,
                          in_flight: &mut HashSet<String>,
                          futures: &mut FuturesUnordered<
            std::pin::Pin<Box<dyn std::future::Future<Output = ScrapeOutcome> + Send>>,
        >| {
            let available: Vec<String> = by_domain
                .keys()
                .filter(|d| !in_flight.contains(*d))
                .cloned()
                .collect();

            for domain in available {
                if in_flight.len() >= concurrency {
                    break;
                }
                let Some(candidate) = by_domain.get_mut(&domain).and_then(|q| q.pop_front()) else {
                    continue;
                };
                // Drop the queue entry once it is drained.
                if by_domain.get(&domain).is_none_or(|q| q.is_empty()) {
                    by_domain.remove(&domain);
                }

                in_flight.insert(domain.clone());
                futures.push(Box::pin(scrape_candidate(
                    self.scraper_service.clone(),
                    Arc::clone(&self.lock_manager),
                    candidate,
                    domain,
                    domain_delay,
                )));
            }
        };

        // Prime the pipeline — fill all slots before entering the drain loop.
        fill_slots(&mut by_domain, &mut in_flight, &mut futures);

        // Drain: as each future completes, free its domain slot and refill.
        while let Some(outcome) = futures.next().await {
            in_flight.remove(&outcome.domain);

            if outcome.errored {
                failed += 1;
            } else if let Some(cmd) = outcome.command {
                succeeded += 1;
                pending_commands.push(cmd);
                // Flush eagerly to bound memory use.
                if pending_commands.len() >= push_batch_size {
                    let batch = std::mem::take(&mut pending_commands);
                    push_service.push(batch).await;
                }
            }

            fill_slots(&mut by_domain, &mut in_flight, &mut futures);
        }

        // Final flush of any remaining products.
        if !pending_commands.is_empty() {
            push_service.push(pending_commands).await;
        }

        let duration_ms = batch_start.elapsed().as_millis() as u64;
        let skipped = total - succeeded - failed;
        info!(
            total,
            succeeded, failed, skipped, duration_ms, "Scraper batch complete"
        );

        self.scraper_perf.record(total as u64, duration_ms);
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

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
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

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
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
    async fn should_scrape_candidates_from_multiple_domains() {
        // Two candidates from different domains — both must be scraped.
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![
                    ScraperCandidate {
                        shop_id: ShopId::new(),
                        shop_name: "Shop A".to_string(),
                        shop_type: ShopType::CommercialDealer,
                        url: url::Url::parse("https://domain-a.com/product/1").unwrap(),
                        main_hash: "h1".to_string(),
                        last_scraped_hash: None,
                    },
                    ScraperCandidate {
                        shop_id: ShopId::new(),
                        shop_name: "Shop B".to_string(),
                        shop_type: ShopType::CommercialDealer,
                        url: url::Url::parse("https://domain-b.com/product/2").unwrap(),
                        main_hash: "h2".to_string(),
                        last_scraped_hash: None,
                    },
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        // Both URLs must be scraped exactly once.
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
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
    async fn should_scrape_all_urls_from_same_domain() {
        // Three candidates all from the same domain — all must be scraped,
        // just sequentially (the pipeline ensures at most one in-flight per
        // domain at a time).
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![
                    ScraperCandidate {
                        shop_id: ShopId::new(),
                        shop_name: "Shop".to_string(),
                        shop_type: ShopType::CommercialDealer,
                        url: url::Url::parse("https://same-domain.com/product/1").unwrap(),
                        main_hash: "h1".to_string(),
                        last_scraped_hash: None,
                    },
                    ScraperCandidate {
                        shop_id: ShopId::new(),
                        shop_name: "Shop".to_string(),
                        shop_type: ShopType::CommercialDealer,
                        url: url::Url::parse("https://same-domain.com/product/2").unwrap(),
                        main_hash: "h2".to_string(),
                        last_scraped_hash: None,
                    },
                    ScraperCandidate {
                        shop_id: ShopId::new(),
                        shop_name: "Shop".to_string(),
                        shop_type: ShopType::CommercialDealer,
                        url: url::Url::parse("https://same-domain.com/product/3").unwrap(),
                        main_hash: "h3".to_string(),
                        last_scraped_hash: None,
                    },
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        // All three URLs must be scraped.
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
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

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
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
