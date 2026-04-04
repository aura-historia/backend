use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::service::shop_registration::ShopRegistrationService;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

#[derive(Clone)]
pub struct CrawlerCronConfig {
    pub spider_interval: Duration,
    pub scraper_interval: Duration,
    pub shop_sync_interval: Duration,
    pub spider_batch_size: i64,
    pub scraper_batch_size: i64,
    pub spider_concurrency: usize,
    pub scraper_concurrency: usize,
    pub spider_classify_threshold: usize,
}

impl Default for CrawlerCronConfig {
    fn default() -> Self {
        Self {
            spider_interval: Duration::from_secs(600), // 10 minutes
            scraper_interval: Duration::from_secs(60), // 1 minute
            shop_sync_interval: Duration::from_secs(10800), // 3 hours
            spider_batch_size: 10,
            scraper_batch_size: 100,
            spider_concurrency: 3,
            scraper_concurrency: 10,
            spider_classify_threshold: 200,
        }
    }
}

#[derive(Clone)]
pub struct CrawlerCronJob {
    config: CrawlerCronConfig,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    scraper_service: Arc<dyn ScraperService>,
    shop_registration: Arc<ShopRegistrationService>,
}

impl CrawlerCronJob {
    pub fn new(
        config: CrawlerCronConfig,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
        shop_registration: ShopRegistrationService,
    ) -> Self {
        Self {
            config,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
            shop_registration: Arc::new(shop_registration),
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
        info!("Spider loop started");
        loop {
            self.run_spider_once().await;
            tokio::time::sleep(self.config.spider_interval).await;
        }
    }

    async fn scraper_loop(&self) {
        info!("Scraper loop started");
        loop {
            self.run_scraper_once().await;
            tokio::time::sleep(self.config.scraper_interval).await;
        }
    }

    async fn shop_sync_loop(&self) {
        info!("Shop sync loop started");
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
                    return;
                }
                let count = candidates.len();

                futures::stream::iter(candidates)
                    .map(|candidate| {
                        let spider_service = self.spider_service.clone();
                        let threshold = self.config.spider_classify_threshold;
                        let shop_url = if candidate.shop_domain.starts_with("http") {
                            candidate.shop_domain.clone()
                        } else {
                            format!("https://{}", candidate.shop_domain)
                        };
                        async move {
                            if let Err(e) = spider_service
                                .run(&candidate.shop_id, &candidate.domain_id, &shop_url, threshold)
                                .await
                            {
                                error!(shop_id = %candidate.shop_id, error = %e, "Spider run failed");
                            }
                        }
                    })
                    .buffer_unordered(self.config.spider_concurrency)
                    .collect::<Vec<()>>()
                    .await;

                info!(count, "Spider tick complete");
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
                    return;
                }
                let count = candidates.len();

                futures::stream::iter(candidates)
                    .map(|candidate| {
                        let scraper_service = self.scraper_service.clone();
                        async move {
                            match scraper_service
                                .scrape(
                                    &candidate.shop_id,
                                    &candidate.url,
                                    &candidate.main_hash,
                                    candidate.last_scraped_hash.as_deref(),
                                )
                                .await
                            {
                                Ok(Some(_normalized_product)) => {}
                                Ok(None) => {}
                                Err(e) => {
                                    error!(shop_id = %candidate.shop_id, url = %candidate.url, error = %e, "Scraper run failed");
                                }
                            }
                        }
                    })
                    .buffer_unordered(self.config.scraper_concurrency)
                    .collect::<Vec<()>>()
                    .await;

                info!(count, "Scraper tick complete");
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
    use crate::service::shop_registration::{
        MockShopRegistrationRepository, MockShopRegistrationSource,
    };
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use common::shop_id::ShopId;

    fn noop_shop_registration() -> ShopRegistrationService {
        let mut source = MockShopRegistrationSource::new();
        source
            .expect_fetch_registered_shops()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let repository = MockShopRegistrationRepository::new();
        ShopRegistrationService::new(Box::new(source), Box::new(repository))
    }

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![SpiderCandidate {
                    shop_id: ShopId::new(),
                    domain_id: uuid::Uuid::new_v4(),
                    shop_domain: "example.com".to_string(),
                }])
            })
        });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _, _| {
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
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
        );

        job.run_spider_once().await;
    }

    #[tokio::test]
    async fn should_run_scraper_candidates() {
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

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_shop_registration(),
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
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            shop_registration,
        );

        job.run_shop_sync_once().await;
    }
}
