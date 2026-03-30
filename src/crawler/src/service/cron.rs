use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
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
            spider_batch_size: 10,
            scraper_batch_size: 100,
            spider_concurrency: 3,
            scraper_concurrency: 10,
            spider_classify_threshold: 200,
        }
    }
}

pub struct CrawlerCronJob {
    config: CrawlerCronConfig,
    spider_candidates: Arc<dyn SpiderCandidateService>,
    spider_service: Arc<dyn SpiderService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    scraper_service: Arc<dyn ScraperService>,
}

impl CrawlerCronJob {
    pub fn new(
        config: CrawlerCronConfig,
        spider_candidates: Box<dyn SpiderCandidateService>,
        spider_service: Box<dyn SpiderService>,
        scraper_candidates: Box<dyn ScraperCandidateService>,
        scraper_service: Box<dyn ScraperService>,
    ) -> Self {
        Self {
            config,
            spider_candidates: spider_candidates.into(),
            spider_service: spider_service.into(),
            scraper_candidates: scraper_candidates.into(),
            scraper_service: scraper_service.into(),
        }
    }

    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");

        let spider_job = self.clone_for_spider();
        let scraper_job = self;

        let spider_handle = tokio::spawn(async move {
            spider_job.spider_loop().await;
        });

        let scraper_handle = tokio::spawn(async move {
            scraper_job.scraper_loop().await;
        });

        let _ = tokio::join!(spider_handle, scraper_handle);
    }

    fn clone_for_spider(&self) -> Self {
        Self {
            config: self.config.clone(),
            spider_candidates: self.spider_candidates.clone(),
            spider_service: self.spider_service.clone(),
            scraper_candidates: self.scraper_candidates.clone(),
            scraper_service: self.scraper_service.clone(),
        }
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
                                .run(&candidate.shop_id, &shop_url, threshold)
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
    use crate::spider::candidate_service::{MockSpiderCandidateService, SpiderCandidate};
    use crate::spider::service::{MockSpiderService, SpiderRunResult};
    use common::shop_id::ShopId;

    #[tokio::test]
    async fn should_run_spider_candidates() {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates.expect_get_candidates().returning(|_| {
            Box::pin(async {
                Ok(vec![SpiderCandidate {
                    shop_id: ShopId::new(),
                    shop_domain: "example.com".to_string(),
                }])
            })
        });

        let mut spider_service = MockSpiderService::new();
        spider_service.expect_run().returning(|_, _, _| {
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
        );

        job.run_scraper_once().await;
    }
}
