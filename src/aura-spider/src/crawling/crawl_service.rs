use std::collections::HashSet;

use spider::tokio;
use spider::website::Website;
use tokio::sync::mpsc;
use tracing::warn;

use crate::error::SpiderError;
use crate::normalization::url::normalize_url;

/// Single crawled page represented by its normalized URL.
pub struct CrawledPage {
    pub url: String,
}

/// Starts crawling `target_url` and streams deduplicated pages via an unbounded channel.
pub async fn start_crawl(
    target_url: &str,
) -> Result<mpsc::UnboundedReceiver<CrawledPage>, SpiderError> {
    let (tx, rx) = mpsc::unbounded_channel::<CrawledPage>();

    let mut website = Website::new(target_url);

    website
        .with_depth(10)
        .with_budget(Some(spider::hashbrown::HashMap::from([("*", 10_000)])))
        .with_respect_robots_txt(false);

    let mut spider_rx = website
        .subscribe(512)
        .ok_or_else(|| SpiderError::Spider("Failed to subscribe to crawl stream".to_string()))?;

    tokio::spawn(async move {
        website.crawl().await;
        website.unsubscribe();
    });

    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;

        let mut seen = HashSet::<String>::new();

        loop {
            match spider_rx.recv().await {
                Ok(page) => {
                    let raw_url = page.get_url().to_string();
                    let normalized = normalize_url(&raw_url);

                    if !seen.insert(normalized.clone()) {
                        continue;
                    }

                    if tx.send(CrawledPage { url: normalized }).is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(dropped)) => {
                    warn!(dropped, "Crawl stream lagged and dropped pages");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

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
