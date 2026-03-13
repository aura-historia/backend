use regex::Regex;
use tracing::{debug, info};

use crate::classification::gemini_client::GeminiClient;
use crate::classification::url_classification_service::{
    filter_product_urls, find_product_url_pattern, matches_product_pattern,
};
use crate::crawling::crawl_service::start_crawl;
use crate::error::SpiderError;

pub struct SpiderService {
    target_url: String,
    classify_threshold: usize,
    gemini_client: GeminiClient,
}

impl SpiderService {
    pub fn new(target_url: String, api_key: String, classify_threshold: usize) -> Self {
        let gemini_client = GeminiClient::new(api_key);
        Self {
            target_url,
            classify_threshold,
            gemini_client,
        }
    }

    pub async fn run(&self) -> Result<Vec<String>, SpiderError> {
        info!(targetUrl = %self.target_url, "Starting crawl");
        info!(
            classifyThreshold = self.classify_threshold,
            "Configured one-time classification threshold"
        );

        let mut crawl_rx = start_crawl(&self.target_url).await?;

        let mut all_urls: Vec<String> = Vec::new();
        let mut total_crawled: usize = 0;
        let mut classification_done = false;
        let mut pattern: Option<Regex> = None;

        while let Some(page) = crawl_rx.recv().await {
            total_crawled += 1;
            all_urls.push(page.url.clone());

            if !classification_done && all_urls.len() >= self.classify_threshold {
                info!(
                    urlCount = all_urls.len(),
                    "Threshold reached, requesting Gemini URL pattern"
                );
                pattern = find_product_url_pattern(&self.gemini_client, &all_urls).await?;
                if pattern.is_none() {
                    return Err(SpiderError::NoProducts(
                        "Gemini found no consistent product URL pattern at threshold classification"
                            .to_string(),
                    ));
                }
                classification_done = true;

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
            pattern = find_product_url_pattern(&self.gemini_client, &all_urls).await?;
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

        let confirmed_products = filter_product_urls(&pattern, &all_urls)?;

        info!(
            confirmedProductCount = confirmed_products.len(),
            "Collected confirmed product URLs"
        );

        Ok(confirmed_products)
    }
}
