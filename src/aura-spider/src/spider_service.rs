use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use crate::classification::link_metadata_repository::{LinkMetadataRepository, SpiderLinkRecord};
use crate::classification::url_classification_service::{
    filter_product_urls, matches_product_pattern,
};
use crate::classification::url_pattern_service::UrlPatternService;
use crate::crawling::crawl_service::{CrawledPage, Crawler};
use crate::error::SpiderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkClass {
    Product,
    Category,
    Imprint,
    Info,
    Other,
}

impl LinkClass {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkClass::Product => "product",
            LinkClass::Category => "category",
            LinkClass::Imprint => "imprint",
            LinkClass::Info => "info",
            LinkClass::Other => "other",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "product" => LinkClass::Product,
            "category" => LinkClass::Category,
            "imprint" => LinkClass::Imprint,
            "info" => LinkClass::Info,
            _ => LinkClass::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledLinkMetadata {
    pub url: String,
    pub class: LinkClass,
    pub hash: String,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub last_scraped: Option<OffsetDateTime>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub links: Vec<CrawledLinkMetadata>,
    pub product_urls: Vec<String>,
    pub product_pattern: Option<String>,
}

impl From<SpiderLinkRecord> for CrawledLinkMetadata {
    fn from(value: SpiderLinkRecord) -> Self {
        Self {
            url: value.url,
            class: LinkClass::from_db(&value.link_class),
            hash: value.main_hash,
            last_scraped: value.last_scraped,
            created: value.created,
            updated: value.updated,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SpiderService: Send + Sync {
    async fn run(
        &self,
        shop_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderError>;
}

pub struct SpiderServiceImpl {
    crawler: Box<dyn Crawler>,
    pattern_service: Box<dyn UrlPatternService>,
    link_metadata_repository: Arc<dyn LinkMetadataRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        crawler: Box<dyn Crawler>,
        pattern_service: Box<dyn UrlPatternService>,
        link_metadata_repository: Arc<dyn LinkMetadataRepository>,
    ) -> Self {
        Self {
            crawler,
            pattern_service,
            link_metadata_repository,
        }
    }

    async fn persist_link_metadata(
        &self,
        shop_url: &str,
        pages: &[CrawledPage],
        pattern: &Option<Regex>,
    ) -> Result<Vec<CrawledLinkMetadata>, SpiderError> {
        let normalized_shop_url = crate::url::normalize_shop_url(shop_url)?;
        let mut metadata = Vec::with_capacity(pages.len());

        for page in pages {
            let link_class = classify_link(&page.url, pattern);
            let record = self
                .link_metadata_repository
                .upsert_link(
                    &normalized_shop_url,
                    &page.url,
                    link_class.as_str(),
                    &page.main_hash,
                )
                .await?;

            metadata.push(record.into());
        }

        Ok(metadata)
    }
}

#[async_trait::async_trait]
impl SpiderService for SpiderServiceImpl {
    async fn run(
        &self,
        shop_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderError> {
        info!(shopUrl = %shop_url, "Starting crawl");
        info!(
            classifyThreshold = classify_threshold,
            "Configured one-time classification threshold"
        );

        let mut crawl_rx = self.crawler.crawl(shop_url).await?;

        let mut all_pages: Vec<CrawledPage> = Vec::new();
        let mut all_urls: Vec<String> = Vec::new();
        let mut total_crawled: usize = 0;

        // Check the store first; only fall back to inference client when nothing is persisted.
        let mut pattern: Option<Regex> = self
            .pattern_service
            .load_pattern_for_shop_url(shop_url)
            .await?;

        // Flag to track whether we have successfully inferred/loaded a pattern
        let mut classification_done = pattern.is_some();
        let mut pattern_loaded_from_store = pattern.is_some();

        if pattern_loaded_from_store {
            info!(
                shopUrl = %shop_url,
                "Loaded persisted product URL pattern"
            );
        }

        // Process pages as they are discovered by the crawler
        while let Some(page) = crawl_rx.recv().await {
            total_crawled += 1;
            all_urls.push(page.url.clone());
            all_pages.push(page.clone());

            // If we don't have a pattern yet, try to infer one once we hit the threshold
            if !classification_done && all_urls.len() >= classify_threshold {
                info!(
                    urlCount = all_urls.len(),
                    "Threshold reached, requesting product URL pattern"
                );

                // Attempt to classify the collected URLs to find the product URL pattern
                pattern = self
                    .pattern_service
                    .classify_and_save(shop_url, &all_urls)
                    .await?;

                if pattern.is_none() {
                    warn!(
                        shopUrl = %shop_url,
                        "Found no product URL pattern at threshold classification"
                    );
                    // Mark as done even if it failed, so we don't repeatedly ask the LLM
                    classification_done = true;
                    pattern_loaded_from_store = false;
                    continue;
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

            // Log whether the current URL matches the pattern
            if classification_done {
                if let Some(last_url) = all_urls.last() {
                    if matches_product_pattern(&pattern, last_url) {
                        debug!(index = total_crawled, url = %last_url, "URL matches product pattern");
                    } else {
                        debug!(
                            index = total_crawled,
                            url = %last_url,
                            "URL does not match product pattern"
                        );
                    }
                }
            } else if let Some(last_url) = all_urls.last() {
                debug!(index = total_crawled, url = %last_url, "Crawled URL");
            }

            // Periodically log progress
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

        // If the crawl finished before hitting the threshold, we still need to classify
        if !classification_done && !all_urls.is_empty() {
            info!(
                urlCount = all_urls.len(),
                "Threshold not reached, classifying collected URLs"
            );
            pattern = self
                .pattern_service
                .classify_and_save(shop_url, &all_urls)
                .await?;
            if pattern.is_none() {
                warn!(
                    shopUrl = %shop_url,
                    "Found no product URL pattern in collected URLs"
                );
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

        // Extract all confirmed product URLs using the inferred/loaded pattern
        let mut confirmed_products = if pattern.is_some() {
            filter_product_urls(&pattern, &all_urls)
        } else {
            Ok(Vec::new())
        };

        // If the stored pattern is stale and fails to match anything, we must reclassify
        if pattern_loaded_from_store && confirmed_products.is_err() {
            warn!(
                shopUrl = %shop_url,
                "Persisted product URL pattern did not match crawl results, reclassifying"
            );
            pattern = self
                .pattern_service
                .classify_and_save(shop_url, &all_urls)
                .await?;
            if pattern.is_none() {
                warn!(
                    shopUrl = %shop_url,
                    "Found no product URL pattern while refreshing persisted pattern"
                );
                confirmed_products = Ok(Vec::new());
            } else {
                confirmed_products = filter_product_urls(&pattern, &all_urls);
            }
        }

        let confirmed_products = confirmed_products.unwrap_or_else(|error| {
            warn!(error = %error, "No confirmed product URLs after classification");
            Vec::new()
        });

        let product_pattern = pattern.as_ref().map(|regex| regex.as_str().to_string());

        info!(
            confirmedProductCount = confirmed_products.len(),
            "Collected confirmed product URLs"
        );

        // Save all discovered links and their classifications to the DB
        let links = self
            .persist_link_metadata(shop_url, &all_pages, &pattern)
            .await?;

        // Mark the shop as crawled
        if let Err(error) = self.pattern_service.mark_as_crawled(shop_url).await {
            warn!(shopUrl = %shop_url, error = %error, "Failed to mark shop as crawled");
        }

        Ok(SpiderRunResult {
            links,
            product_urls: confirmed_products,
            product_pattern,
        })
    }
}

fn classify_link(url: &str, product_pattern: &Option<Regex>) -> LinkClass {
    if matches_product_pattern(product_pattern, url) {
        return LinkClass::Product;
    }

    let lower = url.to_ascii_lowercase();

    if ["imprint", "impressum", "mentions-legales", "legal-notice"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Imprint;
    }

    if ["category", "categorie", "kategorie", "collections", "shop"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Category;
    }

    if ["about", "contact", "faq", "terms", "privacy"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Info;
    }

    LinkClass::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = Regex::new(r"/product/").ok();

        let class = classify_link("https://example.com/product/42", &pattern);

        assert_eq!(class, LinkClass::Product);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/impressum", &pattern);

        assert_eq!(class, LinkClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/collections/modern", &pattern);

        assert_eq!(class, LinkClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/random-page", &pattern);

        assert_eq!(class, LinkClass::Other);
    }

    #[test]
    fn should_map_known_db_value_when_loading_link_class() {
        let class = LinkClass::from_db("category");

        assert_eq!(class, LinkClass::Category);
    }

    #[test]
    fn should_map_unknown_db_value_to_other_when_loading_link_class() {
        let class = LinkClass::from_db("unknown-value");

        assert_eq!(class, LinkClass::Other);
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::classification::link_metadata_repository::MockLinkMetadataRepository;
    use crate::classification::link_metadata_repository::SpiderLinkRecord;
    use crate::classification::url_pattern_service::MockUrlPatternService;
    use crate::crawling::crawl_service::MockCrawler;
    use tokio::sync::mpsc;

    fn setup_mock_link_repo(mock: &mut MockLinkMetadataRepository, call_count: usize) {
        mock.expect_upsert_link()
            .times(call_count)
            .returning(|_, url, _, _| {
                let url_owned = url.to_string();
                Box::pin(async move {
                    Ok(SpiderLinkRecord {
                        shop_url: "https://example.com".to_string(),
                        url: url_owned,
                        link_class: "other".to_string(),
                        main_hash: "hash".to_string(),
                        last_scraped: None,
                        created: OffsetDateTime::now_utc(),
                        updated: OffsetDateTime::now_utc(),
                    })
                })
            });
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, shop_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(mockall::predicate::eq(shop_url))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
    }

    #[tokio::test]
    async fn should_run_spider_and_classify_urls() {
        let mut mock_crawler = MockCrawler::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_link_repo = MockLinkMetadataRepository::new();

        let shop_url = "https://example.com";

        mock_crawler
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                // Send some mock pages
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: "https://example.com/product/1".to_string(),
                            main_hash: "hash1".to_string(),
                        })
                        .await
                        .unwrap();
                    tx_clone
                        .send(CrawledPage {
                            url: "https://example.com/about".to_string(),
                            main_hash: "hash2".to_string(),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        mock_pattern_service
            .expect_load_pattern_for_shop_url()
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 2);

        let service = SpiderServiceImpl::new(
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 1).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls.len(), 1);
        assert_eq!(run_result.product_urls[0], "https://example.com/product/1");
        assert_eq!(run_result.links.len(), 2);
    }

    #[tokio::test]
    async fn should_classify_at_end_if_threshold_not_reached() {
        let mut mock_crawler = MockCrawler::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_link_repo = MockLinkMetadataRepository::new();

        let shop_url = "https://example.com";

        mock_crawler
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: "https://example.com/product/1".to_string(),
                            main_hash: "hash1".to_string(),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        mock_pattern_service
            .expect_load_pattern_for_shop_url()
            .returning(|_| Box::pin(async { Ok(None) }));

        // It should classify at the end because threshold is 10
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 1);

        let service = SpiderServiceImpl::new(
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls.len(), 1);
    }

    #[tokio::test]
    async fn should_reclassify_if_persisted_pattern_fails() {
        let mut mock_crawler = MockCrawler::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_link_repo = MockLinkMetadataRepository::new();

        let shop_url = "https://example.com";

        mock_crawler
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: "https://example.com/item/1".to_string(),
                            main_hash: "hash1".to_string(),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        // Persisted pattern expects /product/
        mock_pattern_service
            .expect_load_pattern_for_shop_url()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        // Reclassification gives the correct /item/ pattern
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));

        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 1);

        let service = SpiderServiceImpl::new(
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls.len(), 1);
        assert_eq!(run_result.product_urls[0], "https://example.com/item/1");
    }
}
