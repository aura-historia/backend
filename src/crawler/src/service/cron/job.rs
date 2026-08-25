use super::config::CrawlerCronConfig;
use super::metrics::PerfCounter;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::product_push::ProductListingPushService;
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::advisory_lock::LocalLockManager;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
#[cfg(test)]
use shop_core::shop_id::ShopId;
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
    pub(super) shop_registration: Arc<ShopRegistrationService>,
    pub(super) product_push: Arc<dyn ProductListingPushService>,
    pub(super) spider_perf: Arc<PerfCounter>,
    pub(super) scraper_perf: Arc<PerfCounter>,
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
        product_push: Box<dyn ProductListingPushService>,
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

    #[tracing::instrument(name = "crawler_run_loop", skip(self))]
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

    #[tracing::instrument(name = "crawler_spider_loop", skip(self))]
    async fn spider_loop(&self) {
        loop {
            self.run_spider_once().await;
            tokio::time::sleep(self.config.spider_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_scraper_loop", skip(self))]
    async fn scraper_loop(&self) {
        loop {
            self.run_scraper_once().await;
            tokio::time::sleep(self.config.scraper_interval).await;
        }
    }

    #[tracing::instrument(name = "crawler_shop_sync_loop", skip(self))]
    async fn shop_sync_loop(&self) {
        loop {
            tokio::time::sleep(self.config.shop_sync_interval).await;
            self.run_shop_sync_once().await;
        }
    }

    #[tracing::instrument(name = "crawler_run_shop_sync_once", skip(self))]
    async fn run_shop_sync_once(&self) {
        match self.shop_registration.sync().await {
            Ok(_) => {}
            Err(e) => warn!(error = %e, "Shop sync failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::MockScraperService;
    use crate::service::cron::test_support::noop_product_push;
    use crate::service::shop_registration::{
        MockShopRegistrationRepository, MockShopRegistrationSource, ShopRegistrationService,
    };
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;
    use shop_core::shop_type::ShopType;

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
                    domains: std::collections::HashSet::from([
                        shop_core::domain::Domain::try_from("test.com").unwrap(),
                    ]),
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
