use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::scraper_service::ScraperService;
use crate::spider::candidate_service::SpiderCandidateService;
use crate::spider::service::SpiderService;
use std::time::Duration;
use tracing::{error, info};

pub struct CrawlerCronConfig {
    pub poll_interval: Duration,
    pub spider_batch_size: i64,
    pub scraper_batch_size: i64,
}

impl Default for CrawlerCronConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(60),
            spider_batch_size: 10,
            scraper_batch_size: 100,
        }
    }
}

pub struct CrawlerCronJob {
    config: CrawlerCronConfig,
    spider_candidates: Box<dyn SpiderCandidateService>,
    spider_service: Box<dyn SpiderService>,
    scraper_candidates: Box<dyn ScraperCandidateService>,
    scraper_service: Box<dyn ScraperService>,
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
            spider_candidates,
            spider_service,
            scraper_candidates,
            scraper_service,
        }
    }

    pub async fn run_loop(self) {
        info!("Starting crawler cron job loop");
        loop {
            self.run_once().await;
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn run_once(&self) {
        // Run Spider
        match self
            .spider_candidates
            .get_candidates(self.config.spider_batch_size)
            .await
        {
            Ok(candidates) => {
                for candidate in candidates {
                    info!(shop_id = %candidate.shop_id, domain = %candidate.shop_domain, "Starting spider for candidate");
                    if let Err(e) = self
                        .spider_service
                        .run(&candidate.shop_id, &candidate.shop_domain, 200)
                        .await
                    {
                        error!(shop_id = %candidate.shop_id, error = %e, "Spider run failed");
                    }
                }
            }
            Err(e) => error!(error = %e, "Failed to retrieve spider candidates"),
        }

        // Run Scraper
        match self
            .scraper_candidates
            .get_candidates(self.config.scraper_batch_size)
            .await
        {
            Ok(candidates) => {
                for candidate in candidates {
                    info!(shop_id = %candidate.shop_id, url = %candidate.url, "Starting scraper for candidate");
                    if let Err(e) = self
                        .scraper_service
                        .scrape(&candidate.shop_id, &candidate.url, &candidate.main_hash)
                        .await
                    {
                        error!(shop_id = %candidate.shop_id, url = %candidate.url, error = %e, "Scraper run failed");
                    }
                }
            }
            Err(e) => error!(error = %e, "Failed to retrieve scraper candidates"),
        }
    }
}
