use bloomfilter::Bloom;
use spider::tokio;
use spider::website::Website;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::spider::utils::url::CrawledUrl;

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
}

#[derive(Debug, Error)]
pub enum SpiderDiscoveryError {
    #[error("Spider discovery error: {0}")]
    Discovery(String),
}

#[derive(Debug, Clone)]
pub struct CrawlerConfig {
    pub delay_millis: u64,
    pub request_timeout_secs: u64,
    pub bloom_capacity: usize,
    pub bloom_fp_rate: f64,
    pub channel_size: usize,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            delay_millis: 500,
            request_timeout_secs: 15,
            bloom_capacity: 100_000,
            bloom_fp_rate: 0.001,
            channel_size: 1000,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait Spider: Send + Sync {
    async fn crawl(
        &self,
        shop_url: &str,
    ) -> Result<mpsc::Receiver<CrawledPage>, SpiderDiscoveryError>;
}

pub struct SpiderImpl {
    config: CrawlerConfig,
}

impl SpiderImpl {
    pub fn new(config: CrawlerConfig) -> Self {
        Self { config }
    }
}

impl Default for SpiderImpl {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(
        &self,
        shop_url: &str,
    ) -> Result<mpsc::Receiver<CrawledPage>, SpiderDiscoveryError> {
        let (tx, rx) = mpsc::channel(self.config.channel_size);

        let mut website = Website::new(shop_url);

        let blacklist_regex = CrawledUrl::blacklist_patterns();

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_respect_robots_txt(true)
            .with_user_agent(Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36"))
            .with_request_timeout(Some(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            )))
            .with_delay(
                std::time::Duration::from_millis(self.config.delay_millis).as_millis() as u64,
            )
            .with_caching(false)
            .build()
            .map_err(|e| SpiderDiscoveryError::Discovery(e.to_string()))?;

        let mut spider_rx = website
            .subscribe(512)
            .ok_or_else(|| SpiderDiscoveryError::Discovery("Failed to subscribe".to_string()))?;

        tokio::spawn(async move {
            website.crawl().await;
            website.unsubscribe();
        });

        let config = self.config.clone();
        tokio::spawn(async move {
            let mut bloom = Bloom::new_for_fp_rate(config.bloom_capacity, config.bloom_fp_rate)
                .expect("bloom filter init failed");

            while let Ok(page) = spider_rx.recv().await {
                let raw_url = page.get_url();

                let normalized = if let Ok(parsed) = url::Url::parse(raw_url) {
                    CrawledUrl::new(parsed)
                } else {
                    continue;
                };

                if normalized.is_blacklisted() {
                    continue;
                }

                let normalized_str = normalized.to_string();

                if !bloom.check(&normalized_str) {
                    bloom.set(&normalized_str);

                    if tx.send(CrawledPage { url: normalized }).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_store_url_when_creating_crawled_page_for_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/product/1").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/product/1");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/about").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/about");
    }
}
