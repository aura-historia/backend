use bloomfilter::Bloom;
use spider::compact_str::CompactString;
use spider::tokio;
use spider::website::Website;
use tokio::sync::mpsc;

use crate::error::SpiderError;
use crate::normalization::url::normalize_url;

const BLACKLIST_URL_SUBSTRINGS: &[&str] = &[
    "?add-to-cart=",
    "&add-to-cart=",
    "?replytocom=",
    "&replytocom=",
    "/wp-admin/",
];

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: String,
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

/// Starts crawling `shop_url` and streams deduplicated pages via an unbounded channel.
pub async fn start_crawl(shop_url: &str) -> Result<mpsc::Receiver<CrawledPage>, SpiderError> {
    // Use a bounded channel to prevent memory exhaustion if consumer is slow
    let (tx, rx) = mpsc::channel(1000);

    let mut website = Website::new(shop_url);

    // This tells the spider engine to ignore any link with these patterns
    // to stop the loop before it starts.
    let blacklist_regex = build_blacklist_url_patterns();

    website
        .with_depth(10)
        .with_blacklist_url(Some(blacklist_regex)) // Prevent fetching junk
        .with_budget(Some(spider::hashbrown::HashMap::from([("*", 10_000)])))
        .with_respect_robots_txt(false);

    let mut spider_rx = website
        .subscribe(512)
        .ok_or_else(|| SpiderError::Spider("Failed to subscribe".to_string()))?;

    tokio::spawn(async move {
        website.crawl().await;
        website.unsubscribe();
    });

    tokio::spawn(async move {
        // Bloom Filter (Memory Management)
        // 100k items capacity with 0.1% false-positive rate
        let mut bloom = Bloom::new_for_fp_rate(100_000, 0.001).expect("bloom filter init failed");

        while let Ok(page) = spider_rx.recv().await {
            let raw_url = page.get_url();

            if is_junk_url(raw_url) {
                continue;
            }

            let normalized = normalize_url(raw_url);

            // Deduplicate
            if !bloom.check(&normalized) {
                bloom.set(&normalized);

                // Send to consumer (waits if channel is full)
                if tx.send(CrawledPage { url: normalized }).await.is_err() {
                    break;
                }
            }
        }
    });

    Ok(rx)
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
        };

        assert_eq!(page.url, "https://example.com/product/1");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: "https://example.com/about".to_string(),
        };

        assert_eq!(page.url, "https://example.com/about");
    }
}
