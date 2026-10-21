use async_trait::async_trait;
use crawler::scraper::candidate_service::ScraperCandidateService;
use crawler::service::cron::{CrawlerCronConfig, CrawlerCronJob};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    ListingSourceRegistrationService, ListingSourceRegistrationSource, ListingSourceSyncError,
    RegisteredListingSource,
};
use crawler::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use crawler::spider::local_lock::LocalLockManager;
use crawler::spider::service::{SpiderRunResult, SpiderService, SpiderServiceError};
use crawler::{CrawlerDomainId, scraper};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use test_api::*;
use tokio::sync::{oneshot, watch};

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

struct AbsentAuthoritativeSource {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ListingSourceRegistrationSource for AbsentAuthoritativeSource {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
}

struct NoWorkSpider {
    started: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl Drop for NoWorkSpider {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl SpiderService for NoWorkSpider {
    async fn run(
        &self,
        _: &ListingSourceId,
        _: &CrawlerDomainId,
        _: &str,
        _: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start spider work")
    }
}

struct NoWorkScraper {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl scraper::scraper_service::ScraperService for NoWorkScraper {
    async fn scrape(
        &self,
        _: scraper::scraper_service::ScrapeRequest,
    ) -> scraper::scraper_service::ScrapeOutcome {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start scraper work")
    }
}

struct NoWorkRawCapture {
    started: Arc<AtomicBool>,
}

#[async_trait]
impl ProductListingRawCaptureService for NoWorkRawCapture {
    async fn capture(
        &self,
        _: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        self.started.store(true, Ordering::SeqCst);
        unreachable!("disabled crawler source must not start raw capture work")
    }
}

fn registered_listing_source(listing_source_id: ListingSourceId) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Crawler admission source")
            .unwrap_or_else(|error| panic!("valid test ListingSource name: {error}")),
        listing_source_slug: ListingSourceSlugId::raw("crawler-admission-source")
            .unwrap_or_else(|error| panic!("valid test ListingSource slug: {error}")),
        crawl_enabled: true,
        fallback_currency: None,
    }
}

#[derive(Clone, Default)]
struct SchedulerProbe {
    spider_started: Arc<AtomicBool>,
    scraper_started: Arc<AtomicBool>,
    raw_capture_started: Arc<AtomicBool>,
    sync_calls: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
}

fn build_scheduler(pool: sqlx::PgPool, probe: &SchedulerProbe) -> CrawlerCronJob {
    let spider_candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let scraper_candidates =
        scraper::candidate_service::ScraperCandidateServiceImpl::new(pool.clone());
    let registration = ListingSourceRegistrationService::new(
        Box::new(AbsentAuthoritativeSource {
            calls: Arc::clone(&probe.sync_calls),
        }),
        Box::new(ListingSourceRegistrationRepositoryImpl::new(pool.clone())),
    );
    CrawlerCronJob::new(
        CrawlerCronConfig {
            spider_interval: Duration::from_millis(1),
            scraper_interval: Duration::from_millis(1),
            spider_concurrency: 1,
            scraper_concurrency: 1,
            ..CrawlerCronConfig::default()
        },
        Arc::new(LocalLockManager::new()),
        Box::new(spider_candidates),
        Box::new(NoWorkSpider {
            started: Arc::clone(&probe.spider_started),
            stopped: Arc::clone(&probe.stopped),
        }),
        Box::new(scraper_candidates),
        Box::new(NoWorkScraper {
            started: Arc::clone(&probe.scraper_started),
        }),
        registration,
        Box::new(NoWorkRawCapture {
            started: Arc::clone(&probe.raw_capture_started),
        }),
    )
}

struct SchedulerStop(watch::Sender<bool>);

impl Drop for SchedulerStop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

fn spawn_scheduler(probe: SchedulerProbe) -> (SchedulerStop, oneshot::Receiver<()>) {
    let context = TestContext::current().expect("crawler scheduler context");
    let (stop, mut stopping) = watch::channel(false);
    let shutdown_stop = stop.clone();
    let stop = SchedulerStop(stop);
    let (join_sender, join_receiver) = oneshot::channel::<std::thread::JoinHandle<()>>();
    let stopped = Arc::clone(&probe.stopped);
    context
        .on_shutdown(async move {
            // Cleanup runs in reverse order; the caller may still own its stop guard.
            shutdown_stop.send_replace(true);
            let thread = join_receiver.await?;
            tokio::task::spawn_blocking(move || thread.join())
                .await?
                .map_err(|_| std::io::Error::other("crawler scheduler thread panicked"))?;
            if !stopped.load(Ordering::SeqCst) {
                return Err(
                    std::io::Error::other("crawler scheduler workers outlived shutdown").into(),
                );
            }
            Ok(())
        })
        .expect("register scheduler thread before spawning");
    let (finished, completion) = oneshot::channel();
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("crawler scheduler runtime");
        runtime.block_on(context.scope(async move {
            let pool = get_postgres_client().await;
            let job = build_scheduler(pool.clone(), &probe);
            tokio::select! {
                _ = job.run_loop() => {},
                _ = stopping.changed() => {},
            }
            pool.close().await;
        }));
        // run_loop drops its three internal JoinHandles when cancelled. Its dedicated
        // runtime must end those workers before context cleanup removes their database.
        drop(runtime);
        let _ = finished.send(());
    });
    if let Err(thread) = join_sender.send(thread) {
        drop(stop);
        let _ = thread.join();
        panic!("crawler scheduler shutdown ownership was lost");
    }
    (stop, completion)
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_disable_absent_source_before_spider_and_scraper_select_work() {
    let pool = get_postgres_client().await;
    let listing_source_id = ListingSourceId::new();
    let registration_repository = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    registration_repository
        .apply_snapshot(&[registered_listing_source(listing_source_id)])
        .await
        .unwrap_or_else(|error| panic!("seed enabled crawler ListingSource: {error}"));
    let domain_id = CrawlerDomainId::new();
    sqlx::query(
        "INSERT INTO listing_source_domains \
         (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
         VALUES ($1, $2, 'crawler-admission.example', 'crawler-admission.example')",
    )
    .bind(domain_id.as_uuid())
    .bind(listing_source_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed crawler domain: {error}"));
    sqlx::query(
        "INSERT INTO listing_source_urls \
         (listing_source_id, domain_id, url, raw_source_record_key, url_class) \
         VALUES ($1, $2, 'https://crawler-admission.example/products/1', \
                 'https://crawler-admission.example/products/1', 'product')",
    )
    .bind(listing_source_id.as_uuid())
    .bind(domain_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("seed crawler product URL: {error}"));

    let spider_candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let scraper_candidates =
        scraper::candidate_service::ScraperCandidateServiceImpl::new(pool.clone());
    assert_eq!(
        1,
        spider_candidates
            .get_candidates(1, &[])
            .await
            .unwrap_or_else(|error| panic!("select seeded spider candidate: {error}"))
            .len()
    );
    assert_eq!(
        1,
        scraper_candidates
            .get_candidates(1, 1, &[])
            .await
            .unwrap_or_else(|error| panic!("select seeded scraper candidate: {error}"))
            .len()
    );

    let probe = SchedulerProbe::default();
    let (stop, completion) = spawn_scheduler(probe.clone());

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let enabled = loop {
        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1",
        )
        .bind(listing_source_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("read refreshed crawler ListingSource: {error}"));
        if !enabled {
            break enabled;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "crawler scheduler did not apply the authoritative empty scope"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(stop);
    completion.await.expect("crawler scheduler runtime stopped");

    assert!(probe.stopped.load(Ordering::SeqCst));
    assert!(!enabled);
    assert!(!probe.spider_started.load(Ordering::SeqCst));
    assert!(!probe.scraper_started.load(Ordering::SeqCst));
    assert!(!probe.raw_capture_started.load(Ordering::SeqCst));
    assert!(
        SpiderCandidateServiceImpl::new(pool.clone())
            .get_candidates(1, &[])
            .await
            .unwrap_or_else(|error| panic!("select disabled spider candidate: {error}"))
            .is_empty()
    );
    assert!(
        scraper::candidate_service::ScraperCandidateServiceImpl::new(pool)
            .get_candidates(1, 1, &[])
            .await
            .unwrap_or_else(|error| panic!("select disabled scraper candidate: {error}"))
            .is_empty()
    );
}

async fn spawn_waiting_scheduler_caller(observing: SchedulerProbe) -> OwnedTask<()> {
    let (ready, started) = oneshot::channel();
    let caller = spawn_owned(async move {
        let (stop, completion) = spawn_scheduler(observing.clone());
        tokio::time::timeout(Duration::from_secs(5), async {
            // Initial sync plus the spider/scraper admission calls prove the inner loops run.
            while observing.sync_calls.load(Ordering::SeqCst) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("scheduler inner loops started");
        ready.send(()).expect("scheduler caller still waiting");
        completion.await.expect("crawler scheduler runtime stopped");
        drop(stop);
    });
    started.await.expect("scheduler caller started");
    caller
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_join_scheduler_workers_when_the_caller_is_cancelled() {
    let probe = SchedulerProbe::default();
    let caller = spawn_waiting_scheduler_caller(probe.clone()).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), async {
        while !probe.stopped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled caller stopped scheduler workers");

    // Keep a second caller running as the test body ends. Reverse cleanup order must
    // stop/join its scheduler before the earlier caller-abort callback is polled.
    drop(spawn_waiting_scheduler_caller(SchedulerProbe::default()).await);
}
