use crate::scraper::candidate_service::{ScraperCandidate, ScraperCandidateService};
use crate::scraper::scraper_service::{ScraperError, ScraperService};
use crate::service::product_push::{ProductPushService, normalize_to_upsert};
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::advisory_lock::{DomainLock, LocalLockManager, UrlLock};
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use crate::{network::policy::NetworkErrorKind, network::policy::retry_cooldown_for};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};

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
    /// Delay between consecutive scraper requests for the same domain.
    pub scraper_domain_delay: Duration,
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
            scraper_domain_delay: Duration::from_secs(1),
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

                let spider_concurrency = self.config.spider_concurrency;
                if spider_concurrency == 0 {
                    warn!("spider_concurrency is 0, skipping spider batch");
                    return;
                }

                let semaphore = Arc::new(Semaphore::new(spider_concurrency));
                let mut join_set: JoinSet<bool> = JoinSet::new();

                for candidate in candidates {
                    let spider_candidates = Arc::clone(&self.spider_candidates);
                    let spider_service = Arc::clone(&self.spider_service);
                    let lock_manager = Arc::clone(&self.lock_manager);
                    let permit_pool = Arc::clone(&semaphore);
                    let threshold = self.config.spider_classify_threshold;
                    let shop_url = if candidate.shop_domain.starts_with("http") {
                        candidate.shop_domain.clone()
                    } else {
                        format!("https://{}", candidate.shop_domain)
                    };

                    join_set.spawn(async move {
                        let Ok(_permit) = permit_pool.acquire_owned().await else {
                            error!("Spider semaphore closed unexpectedly");
                            return false;
                        };

                        let Some(_lock) = DomainLock::try_acquire(&lock_manager, candidate.domain_id)
                        else {
                            warn!(
                                domain_id = %candidate.domain_id,
                                "Skipping domain — lock held by another worker"
                            );
                            return false;
                        };

                        match spider_service
                            .run(&candidate.shop_id, &candidate.domain_id, &shop_url, threshold)
                            .await
                        {
                            Ok(_) => {
                                if let Err(err) =
                                    spider_candidates.reset_crawl_failure(&candidate.domain_id).await
                                {
                                    warn!(
                                        error = %err,
                                        domain = %candidate.shop_domain,
                                        "Failed to reset crawl failure metadata"
                                    );
                                }
                                true
                            }
                            Err(e) => {
                                let cooldown = retry_cooldown_for(NetworkErrorKind::Unknown);
                                let next_crawl_at = time::OffsetDateTime::now_utc()
                                    + time::Duration::seconds(cooldown.as_secs() as i64);
                                if let Err(err) = spider_candidates
                                    .mark_crawl_failure(
                                        &candidate.domain_id,
                                        "spider_run_error",
                                        next_crawl_at,
                                    )
                                    .await
                                {
                                    warn!(
                                        error = %err,
                                        domain = %candidate.shop_domain,
                                        "Failed to persist crawl failure metadata"
                                    );
                                }
                                error!(domain = %candidate.shop_domain, error = %e, "Spider run failed");
                                false
                            }
                        }
                    });
                }

                let mut results: Vec<bool> = Vec::new();
                while let Some(joined) = join_set.join_next().await {
                    match joined {
                        Ok(ok) => results.push(ok),
                        Err(e) => {
                            error!(error = %e, "Spider worker task failed to join");
                            results.push(false);
                        }
                    }
                }

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

struct ScrapeCandidateOutcome {
    command: Option<product::service::product_command::UpsertProductCommand>,
    errored: bool,
    skipped: bool,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
}

async fn scrape_candidate(
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    candidate: ScraperCandidate,
    domain: String,
) -> ScrapeCandidateOutcome {
    let Some(_lock) = UrlLock::try_acquire(&lock_manager, &candidate.url) else {
        warn!(url = %candidate.url, "Skipping URL — lock held by another worker");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        };
    };

    match scraper
        .scrape(
            &candidate.shop_id,
            &candidate.url,
            &candidate.main_hash,
            candidate.last_scraped_hash.as_deref(),
        )
        .await
    {
        Ok(Some(product)) => ScrapeCandidateOutcome {
            command: normalize_to_upsert(product, &candidate),
            errored: false,
            skipped: false,
        },
        Ok(None) => ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        },
        Err(e) => {
            if let ScraperError::HttpError { kind, .. } = &e {
                let cooldown = retry_cooldown_for(*kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(*code as i32),
                    _ => None,
                };
                if let Err(mark_err) = scraper_candidates
                    .mark_fetch_failure(
                        &candidate.shop_id,
                        &candidate.url,
                        &format!("{kind:?}"),
                        status_code,
                        next_retry_at,
                    )
                    .await
                {
                    warn!(
                        error = %mark_err,
                        url = %candidate.url,
                        "Failed to persist scraper failure metadata"
                    );
                }
            }

            error!(
                domain = %domain,
                url = %candidate.url,
                error = %e,
                "Scraper run failed"
            );
            ScrapeCandidateOutcome {
                command: None,
                errored: true,
                skipped: false,
            }
        }
    }
}

async fn scrape_domain_candidates(
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    domain: String,
    candidates: Vec<ScraperCandidate>,
    domain_delay: Duration,
    command_tx: mpsc::UnboundedSender<product::service::product_command::UpsertProductCommand>,
) -> ScrapeDomainOutcome {
    let mut outcome = ScrapeDomainOutcome {
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };

    let len = candidates.len();
    for (idx, candidate) in candidates.into_iter().enumerate() {
        let candidate_outcome = scrape_candidate(
            Arc::clone(&scraper),
            Arc::clone(&scraper_candidates),
            Arc::clone(&lock_manager),
            candidate,
            domain.clone(),
        )
        .await;

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(cmd) = candidate_outcome.command {
            outcome.succeeded += 1;
            if command_tx.send(cmd).is_err() {
                error!("Command channel closed while scraper worker is running");
                outcome.failed += 1;
                outcome.succeeded = outcome.succeeded.saturating_sub(1);
            }
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            outcome.succeeded += 1;
        }

        if idx + 1 < len && !domain_delay.is_zero() {
            tokio::time::sleep(domain_delay).await;
        }
    }

    outcome
}

impl CrawlerCronJob {
    async fn run_scraper_once(&self) {
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
        let scraper_concurrency = self.config.scraper_concurrency;
        if scraper_concurrency == 0 {
            warn!("scraper_concurrency is 0, skipping scraper batch");
            return;
        }

        info!(
            candidates = total,
            concurrency = scraper_concurrency,
            "Scraper batch starting"
        );

        let mut by_domain: HashMap<String, Vec<ScraperCandidate>> = HashMap::new();
        for candidate in all_candidates {
            let domain = candidate.url.host_str().unwrap_or("").to_string();
            by_domain.entry(domain).or_default().push(candidate);
        }

        debug!(domains = by_domain.len(), "Candidates grouped by domain");

        let domain_delay = self.config.scraper_domain_delay;
        let semaphore = Arc::new(Semaphore::new(scraper_concurrency));
        let mut join_set: JoinSet<ScrapeDomainOutcome> = JoinSet::new();
        let (command_tx, mut command_rx) =
            mpsc::unbounded_channel::<product::service::product_command::UpsertProductCommand>();

        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let push_batch_size = self.config.push_batch_size;
        let push_service = Arc::clone(&self.product_push);

        let push_collector = tokio::spawn(async move {
            let mut pending_commands: Vec<product::service::product_command::UpsertProductCommand> =
                Vec::new();

            while let Some(cmd) = command_rx.recv().await {
                pending_commands.push(cmd);
                if pending_commands.len() >= push_batch_size {
                    let batch = std::mem::take(&mut pending_commands);
                    push_service.push(batch).await;
                }
            }

            if !pending_commands.is_empty() {
                push_service.push(pending_commands).await;
            }
        });

        for (domain, candidates) in by_domain {
            let scraper = Arc::clone(&self.scraper_service);
            let scraper_candidates = Arc::clone(&self.scraper_candidates);
            let lock_manager = Arc::clone(&self.lock_manager);
            let permit_pool = Arc::clone(&semaphore);
            let domain_tx = command_tx.clone();

            join_set.spawn(async move {
                let Ok(_permit) = permit_pool.acquire_owned().await else {
                    error!("Scraper semaphore closed unexpectedly");
                    return ScrapeDomainOutcome {
                        succeeded: 0,
                        failed: 1,
                        skipped: 0,
                    };
                };

                scrape_domain_candidates(
                    scraper,
                    scraper_candidates,
                    lock_manager,
                    domain,
                    candidates,
                    domain_delay,
                    domain_tx,
                )
                .await
            });
        }
        drop(command_tx);

        while let Some(joined) = join_set.join_next().await {
            let outcome = match joined {
                Ok(outcome) => outcome,
                Err(e) => {
                    error!(error = %e, "Scraper domain worker task failed to join");
                    failed += 1;
                    continue;
                }
            };

            succeeded += outcome.succeeded;
            failed += outcome.failed;
            skipped += outcome.skipped;
        }

        if let Err(e) = push_collector.await {
            error!(error = %e, "Scraper push collector task failed to join");
            failed += 1;
        }

        let duration_ms = batch_start.elapsed().as_millis() as u64;
        skipped += total.saturating_sub(succeeded + failed + skipped);
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
        spider_candidates
            .expect_reset_crawl_failure()
            .withf(move |domain_id| *domain_id == expected_domain_id)
            .returning(|_| Box::pin(async { Ok(()) }));

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
    async fn should_mark_crawl_failure_when_spider_run_errors() {
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
        spider_candidates
            .expect_mark_crawl_failure()
            .withf(move |domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
            Box::pin(async {
                Err(crate::spider::service::SpiderServiceError::Database(
                    sqlx::Error::RowNotFound,
                ))
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
    async fn should_skip_spider_candidate_when_domain_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        let locked_domain_id = uuid::Uuid::new_v4();
        spider_candidates
            .expect_get_candidates()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(vec![SpiderCandidate {
                        shop_id: ShopId::new(),
                        domain_id: locked_domain_id,
                        shop_domain: "example.com".to_string(),
                    }])
                })
            });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().times(0);

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));

        let scraper_service = MockScraperService::new();
        let lock_manager = Arc::new(LocalLockManager::new());
        let _prelock = DomainLock::try_acquire(&lock_manager, locked_domain_id).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
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
    async fn should_mark_fetch_failure_for_retryable_scraper_http_error() {
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
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|_, url, _, _| {
            let url = url.clone();
            Box::pin(async move {
                Err(ScraperError::HttpError {
                    url,
                    kind: crate::network::policy::NetworkErrorKind::Timeout,
                    details: "timeout".to_string(),
                })
            })
        });

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
    async fn should_scrape_candidates_from_multiple_domains() {
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
    async fn should_skip_scraper_candidate_when_url_lock_is_already_held() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let locked_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let open_url = url::Url::parse("https://domain-a.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_| {
                let locked_url = locked_url.clone();
                let open_url = open_url.clone();
                Box::pin(async move {
                    Ok(vec![
                        ScraperCandidate {
                            shop_id: ShopId::new(),
                            shop_name: "Shop A".to_string(),
                            shop_type: ShopType::CommercialDealer,
                            url: locked_url,
                            main_hash: "h1".to_string(),
                            last_scraped_hash: None,
                        },
                        ScraperCandidate {
                            shop_id: ShopId::new(),
                            shop_name: "Shop A".to_string(),
                            shop_type: ShopType::CommercialDealer,
                            url: open_url,
                            main_hash: "h2".to_string(),
                            last_scraped_hash: None,
                        },
                    ])
                })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));

        let mut push_service = MockProductPushService::new();
        push_service.expect_push().times(0);

        let lock_manager = Arc::new(LocalLockManager::new());
        let prelocked = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let _prelock = UrlLock::try_acquire(&lock_manager, &prelocked).unwrap();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
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
