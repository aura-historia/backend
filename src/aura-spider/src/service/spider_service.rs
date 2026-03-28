use common::shop_id::ShopId;

use std::sync::Arc;

use tracing::{debug, info, warn};

use crate::classification::url_metadata_repository::UrlMetadataRepository;
use crate::classification::url_pattern_service::{UrlPatternService, UrlPatternServiceError};
use crate::discovery::website_spider::{CrawledPage, Spider, SpiderDiscoveryError};
use crate::service::crawl_run_state::CrawlRunState;
use crate::service::product_pattern::ProductPattern;
use crate::utils::url::CrawledUrl;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::classification::url_metadata::UrlClass;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub total_links: usize,
    pub product_urls_count: usize,
    pub product_pattern: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
}

#[derive(Debug, Error)]
pub enum SpiderServiceError {
    #[error(transparent)]
    Discovery(#[from] SpiderDiscoveryError),

    #[error(transparent)]
    UrlPattern(#[from] UrlPatternServiceError),

    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SpiderService: Send + Sync {
    async fn run(
        &self,
        shop_id: &ShopId,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError>;
}

pub struct SpiderServiceImpl {
    config: SpiderServiceConfig,
    shop_url: String,
    spider: Box<dyn Spider>,
    pattern_service: Box<dyn UrlPatternService>,
    url_metadata_repository: Arc<dyn UrlMetadataRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        config: SpiderServiceConfig,
        shop_url: String,
        spider: Box<dyn Spider>,
        pattern_service: Box<dyn UrlPatternService>,
        url_metadata_repository: Arc<dyn UrlMetadataRepository>,
    ) -> Self {
        Self {
            config,
            shop_url,
            spider,
            pattern_service,
            url_metadata_repository,
        }
    }

    async fn persist_url_metadata_batch(
        &self,
        shop_id: &ShopId,
        pages: &[CrawledPage],
        pattern: &ProductPattern,
    ) -> Result<usize, SpiderServiceError> {
        if pages.is_empty() {
            return Ok(0);
        }

        let mut urls = Vec::with_capacity(pages.len());
        let mut classes = Vec::with_capacity(pages.len());
        let mut hashes = Vec::with_capacity(pages.len());

        for page in pages {
            urls.push(page.url.as_url().clone());

            let class_str = classify_url(page.url.as_url().as_str(), pattern).as_str();
            let class = std::str::FromStr::from_str(class_str).unwrap_or(UrlClass::Other);
            classes.push(class);

            hashes.push(page.main_hash.clone());
        }

        if !urls.is_empty() {
            let records = self
                .url_metadata_repository
                .upsert_links_batch(shop_id, &urls, &classes, &hashes)
                .await?;
            Ok(records.len())
        } else {
            Ok(0)
        }
    }

    async fn process_buffer(
        &self,
        buffer: &mut Vec<CrawledPage>,
        shop_id: &ShopId,
        pattern: &ProductPattern,
    ) -> Result<usize, SpiderServiceError> {
        let count = buffer
            .iter()
            .filter(|p| {
                pattern
                    .as_regex()
                    .is_some_and(|regex| p.url.matches_pattern(regex))
            })
            .count();
        self.persist_url_metadata_batch(shop_id, buffer, pattern)
            .await?;
        buffer.clear();
        Ok(count)
    }

    fn count_pattern_matches(pattern: &ProductPattern, urls: &[String]) -> usize {
        let Some(regex) = pattern.as_regex() else {
            return 0;
        };

        urls.iter()
            .filter(|url| {
                if let Ok(parsed) = Url::parse(url) {
                    return CrawledUrl::new(parsed).matches_pattern(regex);
                }
                false
            })
            .count()
    }

    async fn classify_and_save_for_stage(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
        shop_url: &str,
        stage: &'static str,
    ) -> Result<(), SpiderServiceError> {
        state.pattern = self
            .pattern_service
            .classify_and_save(shop_id, shop_url, &state.inference_sample)
            .await
            .map(|pattern| {
                pattern
                    .map(ProductPattern::from)
                    .unwrap_or(ProductPattern::Unknown)
            })?;

        if state.pattern.is_unknown() {
            warn!(shopUrl = %shop_url, stage, "Found no product URL pattern");
        } else {
            let matched_count =
                Self::count_pattern_matches(&state.pattern, &state.inference_sample);
            info!(
                stage,
                matchedCount = matched_count,
                urlCount = state.inference_sample.len(),
                "Classified sample URLs"
            );
        }

        Ok(())
    }

    async fn maybe_classify_at_threshold(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
        shop_url: &str,
        classify_threshold: usize,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && state.total_crawled >= classify_threshold {
            info!(
                urlCount = state.inference_sample.len(),
                "Threshold reached, requesting product URL pattern"
            );

            self.classify_and_save_for_stage(state, shop_id, shop_url, "threshold")
                .await?;

            state.classification_done = true;
            state.pattern_loaded_from_store = false;
        }

        Ok(())
    }

    fn log_page_pattern_state(&self, state: &CrawlRunState, page: &CrawledPage) {
        let current_url = state
            .inference_sample
            .last()
            .cloned()
            .unwrap_or_else(|| page.url.to_string());

        if state.classification_done {
            let is_match = if let Some(regex) = state.pattern.as_regex() {
                if let Ok(parsed) = Url::parse(&current_url) {
                    CrawledUrl::new(parsed).matches_pattern(regex)
                } else {
                    false
                }
            } else {
                false
            };

            if is_match {
                debug!(
                    index = state.total_crawled,
                    url = %current_url,
                    "URL matches product pattern"
                );
            } else {
                debug!(
                    index = state.total_crawled,
                    url = %current_url,
                    "URL does not match product pattern"
                );
            }
        } else {
            debug!(index = state.total_crawled, url = %current_url, "Crawled URL");
        }
    }

    async fn flush_batch_if_needed(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
    ) -> Result<(), SpiderServiceError> {
        if state.classification_done && state.page_buffer.len() >= self.config.db_batch_size {
            state.products_found += self
                .process_buffer(&mut state.page_buffer, shop_id, &state.pattern)
                .await?;
        }
        Ok(())
    }

    fn log_progress(&self, state: &CrawlRunState) {
        if state.total_crawled.is_multiple_of(100) {
            info!(
                totalCrawled = state.total_crawled,
                productsSoFar = state.products_found,
                "Crawl progress"
            );
        }
    }

    async fn classify_at_end_if_needed(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && !state.page_buffer.is_empty() {
            info!(
                urlCount = state.inference_sample.len(),
                "Threshold not reached, classifying collected URLs"
            );

            self.classify_and_save_for_stage(state, shop_id, shop_url, "end_of_crawl")
                .await?;

            state.classification_done = true;
        }
        Ok(())
    }

    async fn reclassify_if_persisted_pattern_failed(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
        shop_url: &str,
    ) -> Result<(), SpiderServiceError> {
        if state.pattern_loaded_from_store
            && state.products_found == 0
            && !state.inference_sample.is_empty()
        {
            warn!(
                shopUrl = %shop_url,
                "Persisted product URL pattern did not match crawl results, reclassifying"
            );

            self.classify_and_save_for_stage(state, shop_id, shop_url, "refresh")
                .await?;
        }

        Ok(())
    }

    async fn flush_remaining_pages(
        &self,
        state: &mut CrawlRunState,
        shop_id: &ShopId,
    ) -> Result<(), SpiderServiceError> {
        if !state.page_buffer.is_empty() {
            state.products_found += self
                .process_buffer(&mut state.page_buffer, shop_id, &state.pattern)
                .await?;
        }
        Ok(())
    }

    async fn mark_as_crawled_best_effort(&self, shop_id: &ShopId, shop_url: &str) {
        if let Err(error) = self
            .pattern_service
            .mark_as_crawled(shop_id, shop_url)
            .await
        {
            warn!(shopUrl = %shop_url, error = %error, "Failed to mark shop as crawled");
        }
    }

    async fn run_locked(
        &self,
        shop_id: &ShopId,
        shop_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let mut crawl_rx = self.spider.crawl(shop_url).await?;

        let initial_pattern = self.pattern_service.load_pattern_for_shop(shop_id).await?;
        let mut state = CrawlRunState::new(initial_pattern);

        if state.pattern_loaded_from_store {
            info!(shopUrl = %shop_url, "Loaded persisted product URL pattern");
        }

        while let Some(page) = crawl_rx.recv().await {
            state.total_crawled += 1;

            if state.inference_sample.len() < self.config.max_sample_urls {
                state.inference_sample.push(page.url.to_string());
            }

            state.page_buffer.push(page.clone());

            self.maybe_classify_at_threshold(&mut state, shop_id, shop_url, classify_threshold)
                .await?;

            self.log_page_pattern_state(&state, &page);
            self.flush_batch_if_needed(&mut state, shop_id).await?;
            self.log_progress(&state);
        }

        info!(totalCrawled = state.total_crawled, "Crawl complete");

        self.classify_at_end_if_needed(&mut state, shop_id, shop_url)
            .await?;
        self.reclassify_if_persisted_pattern_failed(&mut state, shop_id, shop_url)
            .await?;
        self.flush_remaining_pages(&mut state, shop_id).await?;

        let product_pattern = state
            .pattern
            .as_regex()
            .map(|regex| regex.as_str().to_string());

        info!(
            confirmedProductCount = state.products_found,
            "Collected confirmed product URLs"
        );

        self.mark_as_crawled_best_effort(shop_id, shop_url).await;

        Ok(SpiderRunResult {
            total_links: state.total_crawled,
            product_urls_count: state.products_found,
            product_pattern,
        })
    }
}

#[async_trait::async_trait]
impl SpiderService for SpiderServiceImpl {
    async fn run(
        &self,
        shop_id: &ShopId,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let shop_url = self.shop_url.as_str();
        info!(shopUrl = %shop_url, "Starting crawl");
        info!(
            classifyThreshold = classify_threshold,
            "Configured one-time classification threshold"
        );

        if !self
            .pattern_service
            .try_lock_shop(shop_id, shop_url)
            .await?
        {
            warn!(shopUrl = %shop_url, "Shop is already being crawled by another worker");
            return Ok(SpiderRunResult {
                total_links: 0,
                product_urls_count: 0,
                product_pattern: None,
            });
        }

        let run_result = self.run_locked(shop_id, shop_url, classify_threshold).await;

        if let Err(error) = self.pattern_service.unlock_shop(shop_id).await {
            warn!(shopUrl = %shop_url, error = %error, "Failed to release shop crawl lock");
        }

        run_result
    }
}

fn classify_url(url: &str, product_pattern: &ProductPattern) -> UrlClass {
    let crawled = match Url::parse(url) {
        Ok(parsed) => CrawledUrl::new(parsed),
        Err(_) => return UrlClass::Other,
    };

    crawled.classify(product_pattern.as_regex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = ProductPattern::Known(Regex::new(r"/product/").unwrap());

        let class = classify_url("https://example.com/product/42", &pattern);

        assert_eq!(class, UrlClass::Product);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let pattern = ProductPattern::Unknown;

        let class = classify_url("https://example.com/impressum", &pattern);

        assert_eq!(class, UrlClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let pattern = ProductPattern::Unknown;

        let class = classify_url("https://example.com/collections/modern", &pattern);

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let pattern = ProductPattern::Unknown;

        let class = classify_url("https://example.com/random-page", &pattern);

        assert_eq!(class, UrlClass::Other);
    }

    #[test]
    fn should_map_known_db_value_when_loading_url_class() {
        let class = UrlClass::from_db("category");

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_map_unknown_db_value_to_other_when_loading_url_class() {
        let class = UrlClass::from_db("unknown-value");

        assert_eq!(class, UrlClass::Other);
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::classification::url_metadata_repository::MainHash;
    use crate::classification::url_metadata_repository::MockUrlMetadataRepository;
    use crate::classification::url_pattern_service::MockUrlPatternService;
    use crate::discovery::website_spider::MockSpider;
    use regex::Regex;
    use tokio::sync::mpsc;

    fn setup_mock_url_repo(mock: &mut MockUrlMetadataRepository, call_count: usize) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .returning(|_, _, _, _| Box::pin(async move { Ok(Vec::new()) }));
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, shop_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(
                mockall::predicate::always(),
                mockall::predicate::eq(shop_url),
            )
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
    }

    fn setup_mock_lock_lifecycle(mock: &mut MockUrlPatternService, shop_url: &'static str) {
        mock.expect_try_lock_shop()
            .with(
                mockall::predicate::always(),
                mockall::predicate::eq(shop_url),
            )
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(true) }));

        mock.expect_unlock_shop()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
    }

    #[tokio::test]
    async fn should_run_spider_and_classify_urls() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let shop_id: ShopId = uuid::Uuid::new_v4().into();
        let shop_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                // Send some mock pages
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: CrawledUrl::new(
                                Url::parse("https://example.com/product/1").unwrap(),
                            ),
                            main_hash: MainHash("hash1".to_string()),
                        })
                        .await
                        .unwrap();
                    tx_clone
                        .send(CrawledPage {
                            url: CrawledUrl::new(Url::parse("https://example.com/about").unwrap()),
                            main_hash: MainHash("hash2".to_string()),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        mock_pattern_service
            .expect_load_pattern_for_shop()
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_url_repo(&mut mock_url_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            shop_url.to_string(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service.run(&shop_id, 1).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
        assert_eq!(run_result.total_links, 2);
    }

    #[tokio::test]
    async fn should_classify_at_end_if_threshold_not_reached() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let shop_id: ShopId = uuid::Uuid::new_v4().into();
        let shop_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: CrawledUrl::new(
                                Url::parse("https://example.com/product/1").unwrap(),
                            ),
                            main_hash: MainHash("hash1".to_string()),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        mock_pattern_service
            .expect_load_pattern_for_shop()
            .returning(|_| Box::pin(async { Ok(None) }));

        // It should classify at the end because threshold is 10
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_url_repo(&mut mock_url_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            shop_url.to_string(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service.run(&shop_id, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
    }

    #[tokio::test]
    async fn should_reclassify_if_persisted_pattern_fails() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let shop_id: ShopId = uuid::Uuid::new_v4().into();
        let shop_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(shop_url))
            .returning(move |_| {
                let (tx, rx) = mpsc::channel(10);
                let tx_clone = tx.clone();
                tokio::spawn(async move {
                    tx_clone
                        .send(CrawledPage {
                            url: CrawledUrl::new(Url::parse("https://example.com/item/1").unwrap()),
                            main_hash: MainHash("hash1".to_string()),
                        })
                        .await
                        .unwrap();
                });
                Box::pin(async { Ok(rx) })
            });

        // Persisted pattern expects /product/
        mock_pattern_service
            .expect_load_pattern_for_shop()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        // Reclassification gives the correct /item/ pattern
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));

        setup_mock_lock_lifecycle(&mut mock_pattern_service, shop_url);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, shop_url);

        setup_mock_url_repo(&mut mock_url_repo, 1);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            shop_url.to_string(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service.run(&shop_id, 10).await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
    }
}
