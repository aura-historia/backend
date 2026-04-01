use bloomfilter::Bloom;
use sha2::{Digest, Sha256};
use spider::tokio;
use spider::website::Website;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::spider::classification::url_metadata_repository::MainHash;
use crate::spider::utils::url::CrawledUrl;

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
    pub main_hash: MainHash,
}

#[derive(Debug, Error)]
pub enum SpiderDiscoveryError {
    #[error("Spider discovery error: {0}")]
    Discovery(String),
}

fn hash_main_fragment(html: &str, fallback: &str) -> MainHash {
    let content_to_hash = extract_main_fragment(html).unwrap_or(fallback);
    let mut hasher = Sha256::new();
    hasher.update(content_to_hash.as_bytes());
    let digest = hasher.finalize();
    MainHash(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn find_case_insensitive(text: &str, search: &str) -> Option<usize> {
    let search_bytes = search.as_bytes();
    if search_bytes.is_empty() {
        return Some(0);
    }
    text.as_bytes()
        .windows(search_bytes.len())
        .position(|window| window.eq_ignore_ascii_case(search_bytes))
}

fn extract_main_fragment(html: &str) -> Option<&str> {
    let main_start = find_case_insensitive(html, "<main")?;
    let tag_end_rel = html[main_start..].find('>')?;
    let content_start = main_start + tag_end_rel + 1;
    let main_end_rel = find_case_insensitive(&html[content_start..], "</main>")?;
    let content_end = content_start + main_end_rel;
    Some(&html[content_start..content_end])
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
                    let main_hash = hash_main_fragment(&page.get_html(), &normalized_str);
                    bloom.set(&normalized_str);

                    if tx
                        .send(CrawledPage {
                            url: normalized,
                            main_hash,
                        })
                        .await
                        .is_err()
                    {
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
            main_hash: MainHash("abc123".to_string()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/product/1");
        assert_eq!(page.main_hash.0, "abc123");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/about").unwrap()),
            main_hash: MainHash("def456".to_string()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/about");
        assert_eq!(page.main_hash.0, "def456");
    }

    #[test]
    fn should_extract_main_fragment_when_main_tag_exists_for_hashing() {
        let html = "<html><body><main><h1>Hello</h1></main></body></html>";

        let extracted = extract_main_fragment(html);

        assert_eq!(extracted, Some("<h1>Hello</h1>"));
    }

    #[test]
    fn should_use_fallback_when_main_tag_is_missing_for_hashing() {
        let html = "<html><body><section>No main</section></body></html>";
        let fallback = "https://example.com/fallback";

        let hash = hash_main_fragment(html, fallback);
        let fallback_hash = hash_main_fragment("", fallback);

        assert_eq!(hash.0, fallback_hash.0);
    }
}
