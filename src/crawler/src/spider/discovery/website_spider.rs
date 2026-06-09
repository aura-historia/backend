use bloomfilter::Bloom;
use reqwest::redirect::Policy;
use spider::tokio;
use spider::website::Website;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use url::Url;

use crate::spider::utils::url::CrawledUrl;

const SPIDER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36";

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

    async fn resolve_crawl_seed_url(&self, shop_url: &str) -> String {
        let Ok(original_url) = Url::parse(shop_url) else {
            warn!(
                shop_url,
                "Skipping canonical URL resolution for invalid crawl seed"
            );
            return shop_url.to_string();
        };

        let client = match reqwest::Client::builder()
            .redirect(Policy::limited(10))
            .user_agent(SPIDER_USER_AGENT)
            .timeout(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            ))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(shop_url, error = ?error, "Failed to build canonical URL resolver");
                return shop_url.to_string();
            }
        };

        let response = match client.get(original_url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                warn!(shop_url, error = ?error, "Canonical URL resolution failed");
                return shop_url.to_string();
            }
        };

        if !response.status().is_success() {
            warn!(
                shop_url,
                status = %response.status(),
                "Canonical URL resolution returned non-success status"
            );
            return shop_url.to_string();
        }

        let resolved_url = response.url().clone();
        if !is_same_or_www_host(&original_url, &resolved_url) {
            warn!(
                original_url = %original_url,
                resolved_url = %resolved_url,
                "Ignoring canonical URL redirect to unrelated host"
            );
            return shop_url.to_string();
        }

        if original_url != resolved_url {
            debug!(
                original_url = %original_url,
                resolved_url = %resolved_url,
                host_normalized = original_url.host_str() != resolved_url.host_str(),
                "Resolved canonical crawl URL"
            );
        }

        resolved_url.to_string()
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

        let crawl_seed_url = self.resolve_crawl_seed_url(shop_url).await;
        let mut website = Website::new(&crawl_seed_url);

        let blacklist_regex = CrawledUrl::blacklist_patterns();

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_respect_robots_txt(true)
            .with_user_agent(Some(SPIDER_USER_AGENT))
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

fn is_same_or_www_host(original_url: &Url, resolved_url: &Url) -> bool {
    let Some(original_host) = original_url.host_str() else {
        return false;
    };
    let Some(resolved_host) = resolved_url.host_str() else {
        return false;
    };

    strip_www(original_host).eq_ignore_ascii_case(strip_www(resolved_host))
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
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

    #[test]
    fn should_accept_redirect_from_bare_host_to_www_host() {
        let original = Url::parse("https://moeblinger.de").unwrap();
        let resolved = Url::parse("https://www.moeblinger.de/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_accept_redirect_from_www_host_to_bare_host() {
        let original = Url::parse("https://www.example.com").unwrap();
        let resolved = Url::parse("https://example.com/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_subdomain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://shop.example.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_domain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://other.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }
}
