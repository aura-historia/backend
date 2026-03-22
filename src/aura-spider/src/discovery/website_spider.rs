use bloomfilter::Bloom;
use sha2::{Digest, Sha256};
use spider::compact_str::CompactString;
use spider::tokio;
use spider::website::Website;
use tokio::sync::mpsc;

use crate::error::SpiderError;
use crate::utils::url::clean_and_normalize_url;

const BLACKLIST_URL_SUBSTRINGS: &[&str] = &[
    "cart",
    "wishlist",
    "?replytocom=",
    "&replytocom=",
    "/wp-admin/",
    "jpg",
    "pdf",
    "png",
];

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: String,
    pub main_hash: String,
}

fn is_junk_url(url: &str) -> bool {
    BLACKLIST_URL_SUBSTRINGS
        .iter()
        .any(|pattern| url.contains(pattern))
}

fn build_blacklist_url_patterns() -> Vec<CompactString> {
    BLACKLIST_URL_SUBSTRINGS
        .iter()
        .map(|pattern| CompactString::from(regex::escape(pattern)))
        .collect()
}

fn hash_main_fragment(html: &str, fallback: &str) -> String {
    let content_to_hash = extract_main_fragment(html).unwrap_or(fallback);
    let mut hasher = Sha256::new();
    hasher.update(content_to_hash.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
pub trait Crawler: Send + Sync {
    async fn crawl(&self, shop_url: &str) -> Result<mpsc::Receiver<CrawledPage>, SpiderError>;
}

pub struct SpiderCrawler {
    config: CrawlerConfig,
}

impl SpiderCrawler {
    pub fn new(config: CrawlerConfig) -> Self {
        Self { config }
    }
}

impl Default for SpiderCrawler {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

#[async_trait::async_trait]
impl Crawler for SpiderCrawler {
    async fn crawl(&self, shop_url: &str) -> Result<mpsc::Receiver<CrawledPage>, SpiderError> {
        let (tx, rx) = mpsc::channel(self.config.channel_size);

        let mut website = Website::new(shop_url);

        let blacklist_regex = build_blacklist_url_patterns();

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_respect_robots_txt(true)
            .with_request_timeout(Some(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            )))
            .with_delay(
                std::time::Duration::from_millis(self.config.delay_millis).as_millis() as u64,
            )
            .with_caching(false);

        let mut spider_rx = website
            .subscribe(512)
            .ok_or_else(|| SpiderError::Spider("Failed to subscribe".to_string()))?;

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

                if is_junk_url(raw_url) {
                    continue;
                }

                let normalized = clean_and_normalize_url(raw_url);

                if !bloom.check(&normalized) {
                    let main_hash = hash_main_fragment(&page.get_html(), &normalized);
                    bloom.set(&normalized);

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
    use regex::RegexSet;

    #[test]
    fn should_build_blacklist_patterns_matching_junk_urls_for_spider_blacklist() {
        let patterns = build_blacklist_url_patterns();
        let regex_set = RegexSet::new(patterns.iter().map(|pattern| pattern.as_str()))
            .expect("blacklist patterns should compile");

        assert!(regex_set.is_match("https://example.com/product/1?add-to-cart=123"));
        assert!(regex_set.is_match("https://example.com/product/1?a=1&replytocom=456"));
        assert!(regex_set.is_match("https://example.com/wp-admin/admin-ajax.php"));
        assert!(!regex_set.is_match("https://example.com/product/1?a=1&b=2"));
    }

    #[test]
    fn should_return_true_when_url_contains_junk_query_parameter() {
        assert!(is_junk_url("https://example.com/product/1?add-to-cart=123"));
        assert!(is_junk_url(
            "https://example.com/product/1?a=1&replytocom=456"
        ));
    }

    #[test]
    fn should_return_true_when_url_contains_junk_path() {
        assert!(is_junk_url("https://example.com/wp-admin/admin-ajax.php"));
    }

    #[test]
    fn should_return_false_when_url_is_regular_product_url() {
        assert!(!is_junk_url("https://example.com/product/1?a=1&b=2"));
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_product_path() {
        let page = CrawledPage {
            url: "https://example.com/product/1".to_string(),
            main_hash: "abc123".to_string(),
        };

        assert_eq!(page.url, "https://example.com/product/1");
        assert_eq!(page.main_hash, "abc123");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: "https://example.com/about".to_string(),
            main_hash: "def456".to_string(),
        };

        assert_eq!(page.url, "https://example.com/about");
        assert_eq!(page.main_hash, "def456");
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

        assert_eq!(hash, fallback_hash);
    }
}
