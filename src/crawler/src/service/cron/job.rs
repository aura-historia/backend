use super::config::CrawlerCronConfig;
use super::metrics::PerfCounter;
use super::stop::CronStop;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::listing_source_registration::ListingSourceRegistrationService;
use crate::service::raw_capture::ProductListingRawCaptureService;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::local_lock::LocalLockManager;
use crate::spider::service::SpiderService;
#[cfg(test)]
use listing_source_core::ListingSourceId;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Clone)]
pub struct CrawlerCronJob {
    pub(super) config: CrawlerCronConfig,
    pub(super) lock_manager: Arc<LocalLockManager>,
    pub(super) spider_candidates: Arc<dyn SpiderCandidateService>,
    pub(super) spider_service: Arc<dyn SpiderService>,
    pub(super) scraper_candidates: Arc<dyn ScraperCandidateService>,
    pub(super) scraper_service: Arc<dyn ScraperService>,
    pub(super) listing_source_registration: Arc<ListingSourceRegistrationService>,
    pub(super) raw_capture: Arc<dyn ProductListingRawCaptureService>,
    pub(super) spider_perf: Arc<PerfCounter>,
    pub(super) scraper_perf: Arc<PerfCounter>,
    pub(super) stop: CronStop,
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
        listing_source_registration: ListingSourceRegistrationService,
        raw_capture: Box<dyn ProductListingRawCaptureService>,
    ) -> Self {
        Self {
            config,
            lock_manager,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            listing_source_registration: Arc::new(listing_source_registration),
            raw_capture: raw_capture.into(),
            spider_perf: Arc::new(PerfCounter::new(50, "spider")),
            scraper_perf: Arc::new(PerfCounter::new(500, "scraper")),
            stop: CronStop::default(),
        }
    }

    /// The switch that makes [`run_loop`](Self::run_loop) stop starting work and return once
    /// the work already running has finished.
    pub fn stop_handle(&self) -> CronStop {
        self.stop.clone()
    }

    /// Runs the ListingSource sync, spider and scraper loops until a stop is requested through
    /// [`stop_handle`](Self::stop_handle) and the work already running has finished.
    #[tracing::instrument(name = "crawler_run_loop", skip(self))]
    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");

        self.run_listing_source_sync_once().await;

        let spider_job = self.clone();
        let sync_job = self.clone();
        let scraper_job = self;

        let sync_handle = tokio::spawn(async move {
            sync_job.listing_source_sync_loop().await;
        });

        let spider_handle = tokio::spawn(async move {
            spider_job.spider_loop().await;
        });

        let scraper_handle = tokio::spawn(async move {
            scraper_job.scraper_loop().await;
        });

        let _ = tokio::join!(spider_handle, scraper_handle, sync_handle);
    }

    #[tracing::instrument(name = "crawler_spider_loop", skip(self))]
    async fn spider_loop(&self) {
        while !self.stop.is_requested() {
            self.run_spider_once().await;
            self.stop.sleep(self.config.spider_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_scraper_loop", skip(self))]
    async fn scraper_loop(&self) {
        while !self.stop.is_requested() {
            self.run_scraper_once().await;
            self.stop.sleep(self.config.scraper_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_listing_source_sync_loop", skip(self))]
    async fn listing_source_sync_loop(&self) {
        loop {
            self.stop
                .sleep(self.config.listing_source_sync_interval)
                .await;
            if self.stop.is_requested() {
                break;
            }
            self.run_listing_source_sync_once().await;
        }
    }

    #[tracing::instrument(name = "crawler_run_listing_source_sync_once", skip(self))]
    async fn run_listing_source_sync_once(&self) {
        if let Err(error) = self.listing_source_registration.sync().await {
            warn!(
                event = "crawler.listing_source_sync.failed",
                error = %error,
                "ListingSource sync failed"
            );
        }
    }

    pub(super) async fn admit_authoritative_scope_for_work(&self, work_kind: &'static str) -> bool {
        match self.listing_source_registration.sync().await {
            Ok(_) => true,
            Err(error) => {
                warn!(
                    event = "crawler.work_skipped_stale_listing_source_scope",
                    work_kind,
                    error = %error,
                    outcome = "skipped",
                    "crawler work skipped because authoritative ListingSource scope could not be refreshed"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::test_support::{noop_listing_source_registration, noop_raw_capture};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::local_lock::LocalLockManager;
    use crate::spider::service::MockSpiderService;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn should_run_listing_source_sync() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| {
                Box::pin(async {
                    Ok(vec![
                        crate::service::listing_source_registration::RegisteredListingSource {
                            listing_source_id: ListingSourceId::new(),
                            listing_source_name: listing_source_core::ListingSourceName::try_from(
                                "Test source",
                            )
                            .unwrap_or_else(|error| {
                                panic!("invalid test listing source name: {error}")
                            }),
                            listing_source_slug: listing_source_core::ListingSourceSlugId::raw(
                                "test-source",
                            )
                            .unwrap_or_else(|error| {
                                panic!("valid test listing source slug: {error}")
                            }),
                            crawl_enabled: true,
                            fallback_currency: None,
                        },
                    ])
                })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .times(1)
            .returning(|_| {
                Box::pin(async {
                    Ok(crate::service::listing_source_registration::ListingSourceSnapshotResult::default())
                })
            });

        let listing_source_registration =
            ListingSourceRegistrationService::new(Box::new(source), Box::new(repository));

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
            listing_source_registration,
            noop_raw_capture(),
        );

        job.run_listing_source_sync_once().await;
    }

    #[tokio::test(start_paused = true)]
    async fn should_return_from_run_loop_without_a_new_pass_once_stopped() {
        let spider_passes = Arc::new(AtomicUsize::new(0));
        let scraper_passes = Arc::new(AtomicUsize::new(0));
        let mut spider_candidates = MockSpiderCandidateService::new();
        let spider_counter = Arc::clone(&spider_passes);
        spider_candidates
            .expect_get_candidates()
            .returning(move |_, _| {
                // A second pass fails its loop task instead of spinning on the cleared interval.
                let previous_passes = spider_counter.fetch_add(1, Ordering::SeqCst);
                assert_eq!(previous_passes, 0, "a second spider pass started");
                Box::pin(async { Ok(vec![]) })
            });
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_count_due_urls()
            .returning(|| Box::pin(async { Ok(0) }));
        scraper_candidates
            .expect_domains_awaiting_review()
            .returning(|| Box::pin(async { Ok(Vec::new()) }));
        let scraper_counter = Arc::clone(&scraper_passes);
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_, _, _| {
                // A second pass fails its loop task instead of spinning on the cleared interval.
                let previous_passes = scraper_counter.fetch_add(1, Ordering::SeqCst);
                assert_eq!(previous_passes, 0, "a second scraper pass started");
                Box::pin(async { Ok(vec![]) })
            });
        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(MockSpiderService::new()),
            Box::new(scraper_candidates),
            Box::new(MockScraperService::new()),
            noop_listing_source_registration(),
            noop_raw_capture(),
        );
        let stop = job.stop_handle();
        let running = tokio::spawn(job.run_loop());
        while spider_passes.load(Ordering::SeqCst) == 0
            || scraper_passes.load(Ordering::SeqCst) == 0
        {
            tokio::task::yield_now().await;
        }

        stop.request();

        // The loops would otherwise sleep for their whole interval before the next pass.
        tokio::time::timeout(Duration::from_secs(1), running)
            .await
            .expect("run_loop must return once stopped")
            .expect("run_loop must not panic");
        assert_eq!(spider_passes.load(Ordering::SeqCst), 1);
        assert_eq!(scraper_passes.load(Ordering::SeqCst), 1);
    }
}
