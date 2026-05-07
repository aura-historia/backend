use crate::scraper::candidate_service::{
    ProductSnapshot, ScraperCandidate, ScraperCandidateService,
};
use crate::scraper::scraper_service::{
    DEFAULT_MAX_LLM_CALLS_PER_SHOP, DEFAULT_SCHEMA_SEED_PAGES, ScraperError, ScraperService,
};
use crate::service::product_push::{ProductPushService, normalize_to_upsert};
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::advisory_lock::{DomainLock, LocalLockManager, UrlLock};
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use crate::{network::policy::NetworkErrorKind, network::policy::retry_cooldown_for};
use common::shop_id::ShopId;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

/// Context for scraping a domain's candidates.
struct ScrapeDomainContext {
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    domain_delay: Duration,
    command_tx: mpsc::UnboundedSender<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
    budget_exhausted_shops: Arc<Mutex<HashSet<ShopId>>>,
}

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

    #[tracing::instrument(skip(self))]
    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");

        self.run_shop_sync_once().await;

        let spider_job = self.clone();
        let sync_job = self.clone();
        let scraper_job = self;

        let sync_handle = tokio::spawn(async move {
            sync_job.shop_sync_loop().await;
        });

        let spider_handle = tokio::spawn(async move {
            spider_job.spider_loop().await;
        });

        let scraper_handle = tokio::spawn(async move {
            scraper_job.scraper_loop().await;
        });

        let _ = tokio::join!(spider_handle, scraper_handle, sync_handle);
    }

    #[tracing::instrument(skip(self))]
    async fn spider_loop(&self) {
        loop {
            self.run_spider_once().await;
            tokio::time::sleep(self.config.spider_interval).await;
        }
    }

    #[tracing::instrument(skip(self))]
    async fn scraper_loop(&self) {
        loop {
            self.run_scraper_once().await;
            tokio::time::sleep(self.config.scraper_interval).await;
        }
    }

    #[tracing::instrument(skip(self))]
    async fn shop_sync_loop(&self) {
        loop {
            tokio::time::sleep(self.config.shop_sync_interval).await;
            self.run_shop_sync_once().await;
        }
    }

    #[tracing::instrument(skip(self))]
    async fn run_shop_sync_once(&self) {
        match self.shop_registration.sync().await {
            Ok(_) => {}
            Err(e) => warn!(error = %e, "Shop sync failed"),
        }
    }

    #[tracing::instrument(skip(self))]
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
                    warn!(
                        spider_concurrency,
                        "spider_concurrency is 0, skipping spider batch"
                    );
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
                    let span = tracing::info_span!(
                        "spider_candidate",
                        shop_id = %candidate.shop_id,
                        domain_id = %candidate.domain_id,
                        shop_url = %shop_url
                    );

                    join_set.spawn(async move {
                        let Ok(_permit) = permit_pool.acquire_owned().await else {
                            error!("Spider semaphore closed unexpectedly");
                            return false;
                        };

                        let Some(_lock) =
                            DomainLock::try_acquire(&lock_manager, candidate.domain_id)
                        else {
                            warn!(
                                shop_id = %candidate.shop_id,
                                domain_id = %candidate.domain_id,
                                "Skipping domain — lock held by another worker"
                            );
                            return false;
                        };

                        match spider_service
                            .run(
                                &candidate.shop_id,
                                &candidate.domain_id,
                                &shop_url,
                                threshold,
                            )
                            .await
                        {
                            Ok(_) => {
                                if let Err(err) = spider_candidates
                                    .reset_crawl_failure(&candidate.domain_id)
                                    .await
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
                                warn!(domain = %candidate.shop_domain, error = %e, "Spider run failed");
                                false
                            }
                        }
                    }
                    .instrument(span));
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
            Err(e) => warn!(error = %e, "Failed to retrieve spider candidates"),
        }
    }
}

/// Metadata carried alongside a [`UpsertProductCommand`] so the push-collector
/// can call [`ScraperCandidateService::mark_as_scraped`] only after the push
/// has been confirmed.
struct CandidateMeta {
    shop_id: common::shop_id::ShopId,
    url: url::Url,
    hash: String,
    snapshot: ProductSnapshot,
}

struct ScrapeCandidateOutcome {
    command: Option<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
    errored: bool,
    skipped: bool,
}

struct ScrapeDomainOutcome {
    succeeded: usize,
    failed: usize,
    skipped: usize,
}

/// Pushes a batch of `(command, meta)` pairs to the product backend and then
/// calls [`ScraperCandidateService::mark_as_scraped`] for each command that
/// was successfully persisted.
///
/// The position correspondence between `commands` and `metas` is preserved so
/// that succeeded commands can be matched back to their metadata by index.
#[tracing::instrument(
    skip(push_service, scraper_candidates, batch),
    fields(batch_size = batch.len())
)]
async fn flush_batch(
    push_service: &Arc<dyn ProductPushService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<(
        product::service::product_command::UpsertProductCommand,
        CandidateMeta,
    )>,
) {
    let (commands, metas): (Vec<_>, Vec<_>) = batch.into_iter().unzip();

    // Keep a copy of shops_product_ids in order so we can re-match after push.
    let ids_in_order: Vec<String> = commands
        .iter()
        .map(|c| c.shops_product_id.to_string())
        .collect();

    let succeeded = push_service.push(commands).await;
    let succeeded_ids: std::collections::HashSet<String> = succeeded
        .iter()
        .map(|c| c.shops_product_id.to_string())
        .collect();

    for (i, meta) in metas.into_iter().enumerate() {
        if succeeded_ids.contains(&ids_in_order[i])
            && let Err(e) = scraper_candidates
                .mark_as_scraped(&meta.shop_id, &meta.url, &meta.hash, &meta.snapshot)
                .await
        {
            warn!(shop_id = %meta.shop_id, error = %e, url = %meta.url, "Failed to mark product as scraped after push");
        }
    }
}

#[tracing::instrument(
    skip(candidate, ctx),
    fields(
        shop_id = %candidate.shop_id,
        domain = %_domain,
        url = %candidate.url
    )
)]
async fn scrape_candidate(
    candidate: ScraperCandidate,
    _domain: String,
    ctx: &ScrapeDomainContext,
) -> ScrapeCandidateOutcome {
    // Skip URLs from shops with already-exhausted budgets
    {
        let exhausted = ctx.budget_exhausted_shops.lock().await;
        if exhausted.contains(&candidate.shop_id) {
            debug!("Skipping URL — shop LLM budget already exhausted in this batch");
            return ScrapeCandidateOutcome {
                command: None,
                errored: false,
                skipped: true,
            };
        }
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        warn!("Skipping URL — lock held by another worker");
        return ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        };
    };

    match ctx
        .scraper
        .scrape(
            &candidate.shop_id,
            &candidate.url,
            candidate.last_scraped_hash.as_deref(),
        )
        .await
    {
        Ok(Some(scraped)) => {
            let meta = CandidateMeta {
                shop_id: candidate.shop_id,
                url: candidate.url.clone(),
                hash: scraped.hash,
                snapshot: scraped.snapshot,
            };
            ScrapeCandidateOutcome {
                command: normalize_to_upsert(scraped.product, &candidate).map(|cmd| (cmd, meta)),
                errored: false,
                skipped: false,
            }
        }
        Ok(None) => ScrapeCandidateOutcome {
            command: None,
            errored: false,
            skipped: true,
        },
        Err(e) => {
            let error_message = e.to_string();
            let is_llm_budget_exceeded = matches!(&e, ScraperError::LlmBudgetExceeded { .. });

            if let ScraperError::HttpError { kind, .. } = &e {
                let cooldown = retry_cooldown_for(*kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(*code as i32),
                    _ => None,
                };
                if let Err(mark_err) = ctx
                    .scraper_candidates
                    .mark_fetch_failure(
                        &candidate.shop_id,
                        &candidate.url,
                        &format!("{kind:?}"),
                        &error_message,
                        status_code,
                        next_retry_at,
                    )
                    .await
                {
                    warn!(
                        error = %mark_err,
                        "Failed to persist scraper fetch failure metadata"
                    );
                }
            } else {
                // Non-HTTP errors: schema failures, normalization errors, etc.
                // These do not affect retry scheduling but are persisted for observability.
                let error_kind = scraper_error_kind(&e);
                match &e {
                    ScraperError::SchemaRegenerationExhausted { .. }
                    | ScraperError::NormalizationFixExhausted { .. }
                    | ScraperError::LlmBudgetExceeded { .. } => {
                        let cooldown = std::time::Duration::from_secs(30 * 60);
                        let next_retry_at = time::OffsetDateTime::now_utc()
                            + time::Duration::seconds(cooldown.as_secs() as i64);
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_fetch_failure(
                                &candidate.shop_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                None,
                                next_retry_at,
                            )
                            .await
                        {
                            warn!(
                                error = %mark_err,
                                "Failed to persist schema-regeneration/normalization-fix/LLM-budget cooldown metadata"
                            );
                        }
                    }
                    _ => {
                        if let Err(mark_err) = ctx
                            .scraper_candidates
                            .mark_scraper_failure(
                                &candidate.shop_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                            )
                            .await
                        {
                            warn!(
                                error = %mark_err,
                                "Failed to persist scraper failure metadata"
                            );
                        }
                    }
                }
            }

            // Log LLM budget exhaustion at INFO level only once per shop per batch
            if is_llm_budget_exceeded {
                if let ScraperError::LlmBudgetExceeded {
                    shop_id, max_calls, ..
                } = &e
                {
                    let mut exhausted = ctx.budget_exhausted_shops.lock().await;
                    if exhausted.insert(*shop_id) {
                        info!(
                            shop_id = %shop_id,
                            max_calls,
                            "LLM call budget exhausted for shop; skipping remaining URLs in batch"
                        );
                    }
                }
            } else {
                warn!(error = %e, "Scraper run failed");
            }

            ScrapeCandidateOutcome {
                command: None,
                errored: true,
                skipped: false,
            }
        }
    }
}

/// Returns a short, stable, machine-readable kind label for a [`ScraperError`].
///
/// These labels are persisted in `shop_urls.last_error_kind` so that
/// operators can filter / aggregate by error category without having to parse
/// the free-text message.  The `HttpError` variant is included for completeness
/// even though the caller currently only invokes this helper for non-HTTP errors.
fn scraper_error_kind(e: &ScraperError) -> &'static str {
    match e {
        ScraperError::HttpError { .. } => "HttpError",
        ScraperError::ProductRemoved { .. } => "ProductRemoved",
        ScraperError::NoHost { .. } => "NoHost",
        ScraperError::SchemaServiceError(_) => "SchemaServiceError",
        ScraperError::SchemaRegenerationExhausted { .. } => "SchemaRegenerationExhausted",
        ScraperError::NormalizationFixExhausted { .. } => "NormalizationFixExhausted",
        ScraperError::LlmBudgetExceeded { .. } => "LlmBudgetExceeded",
        ScraperError::NormalizationError(_) => "NormalizationError",
    }
}

#[tracing::instrument(
    skip(candidates, ctx),
    fields(domain = %domain, candidate_count = candidates.len())
)]
async fn scrape_domain_candidates(
    domain: String,
    candidates: Vec<ScraperCandidate>,
    ctx: ScrapeDomainContext,
) -> ScrapeDomainOutcome {
    let mut outcome = ScrapeDomainOutcome {
        succeeded: 0,
        failed: 0,
        skipped: 0,
    };

    let len = candidates.len();
    for (idx, candidate) in candidates.into_iter().enumerate() {
        let candidate_outcome = scrape_candidate(candidate, domain.clone(), &ctx).await;

        if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.command {
            outcome.succeeded += 1;
            if ctx.command_tx.send(pair).is_err() {
                error!("Command channel closed while scraper worker is running");
                outcome.failed += 1;
                outcome.succeeded = outcome.succeeded.saturating_sub(1);
            }
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            outcome.succeeded += 1;
        }

        if idx + 1 < len && !ctx.domain_delay.is_zero() {
            tokio::time::sleep(ctx.domain_delay).await;
        }
    }

    outcome
}

impl CrawlerCronJob {
    #[tracing::instrument(skip(self))]
    async fn run_scraper_once(&self) {
        let total_fetch = (self.config.scraper_concurrency as i64) * self.config.scraper_batch_size;

        let all_candidates = match self.scraper_candidates.get_candidates(total_fetch).await {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to retrieve scraper candidates");
                return;
            }
        };

        if all_candidates.is_empty() {
            debug!("No scraper candidates, skipping batch");
            return;
        }

        let total = all_candidates.len();
        let mut unique_shop_ids = std::collections::HashSet::new();
        for candidate in &all_candidates {
            unique_shop_ids.insert(candidate.shop_id);
        }
        let batch_start = tokio::time::Instant::now();
        let scraper_concurrency = self.config.scraper_concurrency;
        if scraper_concurrency == 0 {
            warn!(
                scraper_concurrency,
                "scraper_concurrency is 0, skipping scraper batch"
            );
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
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<(
            product::service::product_command::UpsertProductCommand,
            CandidateMeta,
        )>();

        // Track shops with exhausted LLM budgets to avoid repeated logging
        let budget_exhausted_shops = Arc::new(Mutex::new(HashSet::new()));

        let mut succeeded = 0usize;
        let mut failed = 0usize;
        let mut skipped = 0usize;
        let push_batch_size = self.config.push_batch_size;
        let push_service = Arc::clone(&self.product_push);
        let scraper_candidates_push = Arc::clone(&self.scraper_candidates);

        let push_collector = tokio::spawn(async move {
            let mut pending: Vec<(
                product::service::product_command::UpsertProductCommand,
                CandidateMeta,
            )> = Vec::new();

            while let Some(pair) = command_rx.recv().await {
                pending.push(pair);
                if pending.len() >= push_batch_size {
                    let batch = std::mem::take(&mut pending);
                    flush_batch(&push_service, &scraper_candidates_push, batch).await;
                }
            }

            if !pending.is_empty() {
                flush_batch(&push_service, &scraper_candidates_push, pending).await;
            }
        });

        for (domain, candidates) in by_domain {
            let scraper = Arc::clone(&self.scraper_service);
            let scraper_candidates = Arc::clone(&self.scraper_candidates);
            let lock_manager = Arc::clone(&self.lock_manager);
            let permit_pool = Arc::clone(&semaphore);
            let domain_tx = command_tx.clone();
            let budget_exhausted_shops = Arc::clone(&budget_exhausted_shops);
            let span = tracing::info_span!(
                "scrape_domain",
                domain = %domain
            );

            join_set.spawn(
                async move {
                    let Ok(_permit) = permit_pool.acquire_owned().await else {
                        error!("Scraper semaphore closed unexpectedly");
                        return ScrapeDomainOutcome {
                            succeeded: 0,
                            failed: 1,
                            skipped: 0,
                        };
                    };

                    let ctx = ScrapeDomainContext {
                        scraper,
                        scraper_candidates,
                        lock_manager,
                        domain_delay,
                        command_tx: domain_tx,
                        budget_exhausted_shops,
                    };

                    scrape_domain_candidates(domain, candidates, ctx).await
                }
                .instrument(span),
            );
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

        #[cfg(not(test))]
        {
            match self
                .scraper_candidates
                .get_shop_llm_usage(unique_shop_ids.into_iter().collect())
                .await
            {
                Ok(usages) => {
                    for usage in usages {
                        info!(
                            shop_name = %usage.shop_name,
                            llm_calls_count = usage.llm_calls_count,
                            llm_calls_cap = self.config.scraper_max_llm_calls_per_shop,
                            llm_budget_exhausted = usage.llm_calls_count >= self.config.scraper_max_llm_calls_per_shop,
                            "Shop LLM usage summary"
                        );
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load per-shop LLM usage summary");
                }
            }
        }

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
        push.expect_push()
            .returning(|cmds| Box::pin(async move { cmds }));
        Box::new(push)
    }

    fn scraper_candidate(shop_name: &str, shop_type: ShopType, url: url::Url) -> ScraperCandidate {
        ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: shop_name.to_string(),
            shop_type,
            url,
            last_scraped_hash: None,
            last_scraped_price: None,
            last_scraped_price_estimate_min: None,
            last_scraped_price_estimate_max: None,
            last_scraped_url: None,
            last_scraped_images_hash: None,
            last_scraped_auction_start: None,
            last_scraped_auction_end: None,
            last_scraped_state: None,
        }
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
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

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
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(|_, url, _| {
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
    async fn should_mark_fetch_failure_for_llm_budget_exceeded_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|shop_id, url, _| {
                let url = url.clone();
                let shop_id = *shop_id;
                Box::pin(async move {
                    Err(ScraperError::LlmBudgetExceeded {
                        shop_id,
                        url,
                        max_calls: 5,
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

    /// `NormalizationFixExhausted` must be handled identically to
    /// `SchemaRegenerationExhausted`: write a cooldown via `mark_fetch_failure`
    /// so the URL is held back until the backoff window expires.
    #[tokio::test]
    async fn should_mark_fetch_failure_for_normalization_fix_exhausted_error() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let spider_service = MockSpiderService::new();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "Test Shop",
                    ShopType::CommercialDealer,
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )])
            })
        });
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(()) }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::NormalizationFixExhausted {
                        url,
                        attempts: 3,
                        last_norm_error: crate::scraper::normalization::product_normalization_service::NormalizationError::TitleEmpty,
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
                    scraper_candidate(
                        "Shop A",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://domain-a.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop B",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://domain-b.com/product/2").unwrap(),
                    ),
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

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
                        scraper_candidate("Shop A", ShopType::CommercialDealer, locked_url),
                        scraper_candidate("Shop A", ShopType::CommercialDealer, open_url),
                    ])
                })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

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
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/2").unwrap(),
                    ),
                    scraper_candidate(
                        "Shop",
                        ShopType::CommercialDealer,
                        url::Url::parse("https://same-domain.com/product/3").unwrap(),
                    ),
                ])
            })
        });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_, _, _| Box::pin(async { Ok(None) }));

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
