use regex::Regex;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::classification::gemini_client::GeminiClient;
use crate::classification::url_classification_service::{
    filter_product_urls, matches_product_pattern,
};
use crate::classification::url_pattern_repository::ShopUrlPatternRepository;
use crate::classification::url_pattern_service::UrlPatternService;
use crate::crawling::crawl_service::start_crawl;
use crate::error::SpiderError;

pub struct SpiderService {
    shop_url: String,
    classify_threshold: usize,
    gemini_client: GeminiClient,
    repository: Arc<dyn ShopUrlPatternRepository>,
}

impl SpiderService {
    pub fn new(
        shop_url: String,
        api_key: String,
        classify_threshold: usize,
        repository: Arc<dyn ShopUrlPatternRepository>,
    ) -> Self {
        let gemini_client = GeminiClient::new(api_key);
        Self {
            shop_url,
            classify_threshold,
            gemini_client,
            repository,
        }
    }

    pub async fn run(&self) -> Result<Vec<String>, SpiderError> {
        info!(shopUrl = %self.shop_url, "Starting crawl");
        info!(
            classifyThreshold = self.classify_threshold,
            "Configured one-time classification threshold"
        );

        let mut crawl_rx = start_crawl(&self.shop_url).await?;

        let pattern_service = UrlPatternService::new(self.repository.clone());

        let mut all_urls: Vec<String> = Vec::new();
        let mut total_crawled: usize = 0;

        // Check the store first; only fall back to Gemini when nothing is persisted.
        let mut pattern: Option<Regex> = pattern_service
            .load_pattern_for_shop_url(&self.shop_url)
            .await?;
        let mut classification_done = pattern.is_some();
        let mut pattern_loaded_from_store = pattern.is_some();

        if pattern_loaded_from_store {
            info!(
                shopUrl = %self.shop_url,
                "Loaded persisted product URL pattern"
            );
        }

        while let Some(page) = crawl_rx.recv().await {
            total_crawled += 1;
            all_urls.push(page.url.clone());

            if !classification_done && all_urls.len() >= self.classify_threshold {
                info!(
                    urlCount = all_urls.len(),
                    "Threshold reached, requesting Gemini URL pattern"
                );
                pattern = pattern_service
                    .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                    .await?;
                if pattern.is_none() {
                    return Err(SpiderError::NoProducts(
                        "Gemini found no consistent product URL pattern at threshold classification"
                            .to_string(),
                    ));
                }
                classification_done = true;
                pattern_loaded_from_store = false;

                let matched_count = all_urls
                    .iter()
                    .filter(|url| matches_product_pattern(&pattern, url))
                    .count();
                info!(
                    matchedCount = matched_count,
                    urlCount = all_urls.len(),
                    "Classified threshold sample URLs"
                );
            }

            if classification_done {
                if matches_product_pattern(&pattern, &page.url) {
                    debug!(index = total_crawled, url = %page.url, "URL matches product pattern");
                } else {
                    debug!(
                        index = total_crawled,
                        url = %page.url,
                        "URL does not match product pattern"
                    );
                }
            } else {
                debug!(index = total_crawled, url = %page.url, "Crawled URL");
            }

            if total_crawled.is_multiple_of(100) {
                let products_so_far = all_urls
                    .iter()
                    .filter(|url| matches_product_pattern(&pattern, url))
                    .count();
                info!(
                    totalCrawled = total_crawled,
                    productsSoFar = products_so_far,
                    "Crawl progress"
                );
            }
        }

        info!(totalCrawled = total_crawled, "Crawl complete");

        if !classification_done && !all_urls.is_empty() {
            info!(
                urlCount = all_urls.len(),
                "Threshold not reached, classifying collected URLs"
            );
            pattern = pattern_service
                .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                .await?;
            if pattern.is_none() {
                return Err(SpiderError::NoProducts(
                    "Gemini found no consistent product URL pattern in collected URLs".to_string(),
                ));
            }

            let matched_count = all_urls
                .iter()
                .filter(|url| matches_product_pattern(&pattern, url))
                .count();
            info!(
                matchedCount = matched_count,
                urlCount = all_urls.len(),
                "Classified collected URLs"
            );
        }

        if pattern.is_none() {
            return Err(SpiderError::NoProducts(
                "Gemini found no consistent product URL pattern in the crawled URLs".to_string(),
            ));
        }

        let mut confirmed_products = filter_product_urls(&pattern, &all_urls);

        if pattern_loaded_from_store && confirmed_products.is_err() {
            warn!(
                shopUrl = %self.shop_url,
                "Persisted product URL pattern did not match crawl results, reclassifying"
            );
            pattern = pattern_service
                .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                .await?;
            if pattern.is_none() {
                return Err(SpiderError::NoProducts(
                    "Gemini found no consistent product URL pattern while refreshing persisted pattern"
                        .to_string(),
                ));
            }

            confirmed_products = filter_product_urls(&pattern, &all_urls);
        }

        let confirmed_products = confirmed_products?;

        info!(
            confirmedProductCount = confirmed_products.len(),
            "Collected confirmed product URLs"
        );

        Ok(confirmed_products)
    }
}
