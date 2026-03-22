use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use crate::classification::link_metadata_repository::LinkMetadataRepository;
use crate::classification::url_classification_service::matches_product_pattern;
use crate::classification::url_pattern_service::UrlPatternService;
use crate::discovery::website_spider::{CrawledPage, Crawler};
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

    pub fn from_db(value: &str) -> Self {
        match value {
            "product" => LinkClass::Product,
            "category" => LinkClass::Category,
            "imprint" => LinkClass::Imprint,
            "info" => LinkClass::Info,
            _ => LinkClass::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl ProductState {
    pub fn from_db(value: &str) -> Self {
        match value {
            "LISTED" => ProductState::Listed,
            "AVAILABLE" => ProductState::Available,
            "RESERVED" => ProductState::Reserved,
            "SOLD" => ProductState::Sold,
            "REMOVED" => ProductState::Removed,
            _ => ProductState::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledLinkMetadata {
    pub url: String,
    pub class: LinkClass,
    pub hash: String,
    pub state: ProductState,

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
    pub total_links: usize,
    pub product_urls_count: usize,
    pub product_pattern: Option<String>,
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

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
        }
    }
}

pub struct SpiderServiceImpl {
    config: SpiderServiceConfig,
    crawler: Box<dyn Crawler>,
    pattern_service: Box<dyn UrlPatternService>,
    link_metadata_repository: Arc<dyn LinkMetadataRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        config: SpiderServiceConfig,
        crawler: Box<dyn Crawler>,
        pattern_service: Box<dyn UrlPatternService>,
        link_metadata_repository: Arc<dyn LinkMetadataRepository>,
    ) -> Self {
        Self {
            config,
            crawler,
            pattern_service,
            link_metadata_repository,
        }
    }

    async fn persist_link_metadata_batch(
        &self,
        shop_url: &str,
        pages: &[CrawledPage],
        pattern: &Option<Regex>,
    ) -> Result<usize, SpiderError> {
        if pages.is_empty() {
            return Ok(0);
        }

        let normalized_shop_url = crate::utils::url::extract_shop_base_url(shop_url)?;

        let mut urls = Vec::with_capacity(pages.len());
        let mut classes = Vec::with_capacity(pages.len());
        let mut hashes = Vec::with_capacity(pages.len());

        for page in pages {
            urls.push(page.url.clone());
            classes.push(classify_link(&page.url, pattern).as_str().to_string());
            hashes.push(page.main_hash.clone());
        }

        let records = self
            .link_metadata_repository
            .upsert_links_batch(&normalized_shop_url, &urls, &classes, &hashes)
            .await?;

        Ok(records.len())
    }

    async fn process_buffer(
        &self,
        buffer: &mut Vec<CrawledPage>,
        shop_url: &str,
        pattern: &Option<Regex>,
    ) -> Result<usize, SpiderError> {
        let count = buffer
            .iter()
            .filter(|p| matches_product_pattern(pattern, &p.url))
            .count();
        self.persist_link_metadata_batch(shop_url, buffer, pattern)
            .await?;
        buffer.clear();
        Ok(count)
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

        if !self.pattern_service.try_lock_shop(shop_url).await? {
            warn!(shopUrl = %shop_url, "Shop is already being crawled by another worker");
            return Ok(SpiderRunResult {
                total_links: 0,
                product_urls_count: 0,
                product_pattern: None,
            });
        }

        let run_result = async {
            let mut crawl_rx = self.crawler.crawl(shop_url).await?;

            let mut total_crawled: usize = 0;
            let mut products_found: usize = 0;

            let mut pattern: Option<Regex> = self
                .pattern_service
                .load_pattern_for_shop_url(shop_url)
                .await?;

            let mut classification_done = pattern.is_some();
            let mut pattern_loaded_from_store = pattern.is_some();

            if pattern_loaded_from_store {
                info!(
                    shopUrl = %shop_url,
                    "Loaded persisted product URL pattern"
                );
            }

            let mut page_buffer: Vec<CrawledPage> = Vec::new();
            let mut inference_sample: Vec<String> = Vec::new();

            while let Some(page) = crawl_rx.recv().await {
                total_crawled += 1;

                if inference_sample.len() < self.config.max_sample_urls {
                    inference_sample.push(page.url.clone());
                }

                page_buffer.push(page.clone());

                if !classification_done && total_crawled >= classify_threshold {
                    info!(
                        urlCount = inference_sample.len(),
                        "Threshold reached, requesting product URL pattern"
                    );

                    pattern = self
                        .pattern_service
                        .classify_and_save(shop_url, &inference_sample)
                        .await?;

                    if pattern.is_none() {
                        warn!(
                            shopUrl = %shop_url,
                            "Found no product URL pattern at threshold classification"
                        );
                    } else {
                        let matched_count = inference_sample
                            .iter()
                            .filter(|url| matches_product_pattern(&pattern, url))
                            .count();
                        info!(
                            matchedCount = matched_count,
                            urlCount = inference_sample.len(),
                            "Classified threshold sample URLs"
                        );
                    }

                    classification_done = true;
                    pattern_loaded_from_store = false;
                }

                if classification_done {
                    if let Some(last_url) = inference_sample.last().or(Some(&page.url)) {
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

                    if page_buffer.len() >= self.config.db_batch_size {
                        products_found +=
                            self.process_buffer(&mut page_buffer, shop_url, &pattern).await?;
                    }
                } else if let Some(last_url) = inference_sample.last().or(Some(&page.url)) {
                    debug!(index = total_crawled, url = %last_url, "Crawled URL");
                }

                if total_crawled.is_multiple_of(100) {
                    info!(
                        totalCrawled = total_crawled,
                        productsSoFar = products_found,
                        "Crawl progress"
                    );
                }
            }

            info!(totalCrawled = total_crawled, "Crawl complete");

            if !classification_done && !page_buffer.is_empty() {
                info!(
                    urlCount = inference_sample.len(),
                    "Threshold not reached, classifying collected URLs"
                );
                pattern = self
                    .pattern_service
                    .classify_and_save(shop_url, &inference_sample)
                    .await?;
                if pattern.is_none() {
                    warn!(
                        shopUrl = %shop_url,
                        "Found no product URL pattern in collected URLs"
                    );
                } else {
                    let matched_count = inference_sample
                        .iter()
                        .filter(|url| matches_product_pattern(&pattern, url))
                        .count();
                    info!(
                        matchedCount = matched_count,
                        urlCount = inference_sample.len(),
                        "Classified collected URLs"
                    );
                }
            }

            if pattern_loaded_from_store && products_found == 0 && !inference_sample.is_empty() {
                warn!(
                    shopUrl = %shop_url,
                    "Persisted product URL pattern did not match crawl results, reclassifying"
                );
                pattern = self
                    .pattern_service
                    .classify_and_save(shop_url, &inference_sample)
                    .await?;
                if pattern.is_none() {
                    warn!(
                        shopUrl = %shop_url,
                        "Found no product URL pattern while refreshing persisted pattern"
                    );
                } else {
                    let matched_count = inference_sample
                        .iter()
                        .filter(|url| matches_product_pattern(&pattern, url))
                        .count();
                    info!(
                        matchedCount = matched_count,
                        urlCount = inference_sample.len(),
                        "Classified refreshed collected URLs"
                    );
                }
            }

            if !page_buffer.is_empty() {
                products_found += self.process_buffer(&mut page_buffer, shop_url, &pattern).await?;
            }

            let product_pattern = pattern.as_ref().map(|regex| regex.as_str().to_string());

            info!(
                confirmedProductCount = products_found,
                "Collected confirmed product URLs"
            );

            if let Err(error) = self.pattern_service.mark_as_crawled(shop_url).await {
                warn!(shopUrl = %shop_url, error = %error, "Failed to mark shop as crawled");
            }

            Ok(SpiderRunResult {
                total_links: total_crawled,
                product_urls_count: products_found,
                product_pattern,
            })
        }
        .await;

        if let Err(error) = self.pattern_service.unlock_shop(shop_url).await {
            warn!(shopUrl = %shop_url, error = %error, "Failed to release shop crawl lock");
        }

        run_result
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
    use crate::classification::url_pattern_service::MockUrlPatternService;
    use crate::discovery::website_spider::MockCrawler;
    use tokio::sync::mpsc;

    fn setup_mock_link_repo(mock: &mut MockLinkMetadataRepository, call_count: usize) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .returning(|_, _, _, _| Box::pin(async move { Ok(Vec::new()) }));
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, shop_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(mockall::predicate::eq(shop_url))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
    }

    fn setup_mock_lock_lifecycle(mock: &mut MockUrlPatternService, shop_url: &'static str) {
        mock.expect_try_lock_shop()
            .with(mockall::predicate::eq(shop_url))
            .times(1)
            .returning(|_| Box::pin(async { Ok(true) }));

        mock.expect_unlock_shop()
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

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 1).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
        assert_eq!(run_result.total_links, 2);
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

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
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

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_link_repo(&mut mock_link_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_crawler),
            Box::new(mock_pattern_service),
            Arc::new(mock_link_repo),
        );

        let result = service.run(shop_url, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
    }
}
