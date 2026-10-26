use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use std::sync::Arc;

use crate::network::policy::is_same_or_www_host;
use crate::spider::classification::url_metadata_repository::UrlMetadataRepository;
use crate::spider::classification::url_pattern_service::{
    UrlPatternService, UrlPatternServiceError,
};
use crate::spider::discovery::website_spider::{
    CrawlCompletion, CrawlDiagnostics, CrawlFailureKind, CrawledPage, Spider, SpiderDiscoveryError,
};
use crate::spider::service::crawl_run_state::CrawlRunState;
use crate::spider::service::product_pattern::ProductListingPattern;
use crate::spider::utils::url::CrawledUrl;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use url::Url;

use crate::spider::classification::url_metadata::UrlClass;
use thiserror::Error;

/// URLs from every outcome are persisted; `completion` tells the scheduler
/// whether the crawl was full or must be treated as truncated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub total_links: usize,
    pub product_urls_count: usize,
    pub product_pattern: Option<String>,
    pub completion: CrawlCompletion,
    /// The pending URL-pattern review the crawl stopped classifying for. Its
    /// URLs are stored without a product class until the review is decided,
    /// so the run is neither a success nor a failure.
    pub pending_url_pattern_review: Option<CrawlerReviewId>,
}

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
    pub min_inference_sample_urls: usize,
}

#[derive(Debug, Error)]
pub enum SpiderServiceError {
    #[error(transparent)]
    Discovery(#[from] SpiderDiscoveryError),

    #[error(transparent)]
    UrlPattern(#[from] UrlPatternServiceError),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    UrlMetadata(
        Box<crate::spider::classification::url_metadata_repository::UrlMetadataRepositoryError>,
    ),

    /// The crawl produced too few usable pages; `kind` explains why and
    /// `diagnostics` carries the bounded library and local-filter evidence.
    #[error(
        "Spider crawl failed for crawl-root URL '{crawl_root_url}' with kind '{kind}' after {total_links} page(s)"
    )]
    CrawlFailed {
        crawl_root_url: String,
        kind: CrawlFailureKind,
        total_links: usize,
        diagnostics: Box<CrawlDiagnostics>,
    },

    #[error(
        "Cannot classify product URL pattern for crawl-root URL '{crawl_root_url}' at stage '{stage}' because the inference sample has {sample_size} URL(s), minimum is {min_sample_size}"
    )]
    InsufficientInferenceSample {
        crawl_root_url: String,
        stage: &'static str,
        sample_size: usize,
        min_sample_size: usize,
    },

    #[error(
        "Cannot classify product URL pattern for crawl-root URL '{crawl_root_url}' at stage '{stage}' because the inference sample is empty"
    )]
    EmptyClassificationSample {
        crawl_root_url: String,
        stage: &'static str,
    },

    /// The crawl delivered pages, but the domain's product URL pattern
    /// matched none of them: the pattern is stale or wrong for this site.
    #[error(
        "Spider crawl for crawl-root URL '{crawl_root_url}' found no product URL among {total_links} page(s) with pattern '{product_pattern}'"
    )]
    NoProductUrls {
        crawl_root_url: String,
        total_links: usize,
        product_pattern: String,
        completion: CrawlCompletion,
    },

    /// The crawl delivered pages, but the domain's URL classification is
    /// complete without a product pattern, so no page can become a product
    /// URL until an operator decides or resets the pattern.
    #[error(
        "Spider crawl for crawl-root URL '{crawl_root_url}' found {total_links} page(s) but the domain has no product URL pattern"
    )]
    NoUrlPattern {
        crawl_root_url: String,
        total_links: usize,
        completion: CrawlCompletion,
    },
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
            min_inference_sample_urls: 20,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SpiderService: Send + Sync {
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        _crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError>;
}

pub struct SpiderServiceImpl {
    config: SpiderServiceConfig,
    spider: Box<dyn Spider>,
    pattern_service: Box<dyn UrlPatternService>,
    url_metadata_repository: Arc<dyn UrlMetadataRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        config: SpiderServiceConfig,
        spider: Box<dyn Spider>,
        pattern_service: Box<dyn UrlPatternService>,
        url_metadata_repository: Arc<dyn UrlMetadataRepository>,
    ) -> Self {
        Self {
            config,
            spider,
            pattern_service,
            url_metadata_repository,
        }
    }

    async fn persist_url_metadata_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pages: &[CrawledPage],
        pattern: &ProductListingPattern,
    ) -> Result<usize, SpiderServiceError> {
        if pages.is_empty() {
            return Ok(0);
        }

        let mut urls = Vec::with_capacity(pages.len());
        let mut classes = Vec::with_capacity(pages.len());

        for page in pages {
            urls.push(page.url.as_url().clone());

            let class_str = classify_url(page.url.as_url().as_str(), pattern).as_str();
            let class = std::str::FromStr::from_str(class_str).unwrap_or(UrlClass::Other);
            classes.push(class);
        }

        if !urls.is_empty() {
            let records = self
                .url_metadata_repository
                .upsert_links_batch(listing_source_id, domain_id, &urls, &classes)
                .await
                .map_err(|error| SpiderServiceError::UrlMetadata(Box::new(error)))?;
            Ok(records.len())
        } else {
            Ok(0)
        }
    }

    async fn process_buffer(
        &self,
        buffer: &mut Vec<CrawledPage>,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &ProductListingPattern,
    ) -> Result<usize, SpiderServiceError> {
        let count = count_products(buffer, pattern);
        self.persist_url_metadata_batch(listing_source_id, domain_id, buffer, pattern)
            .await?;
        buffer.clear();
        Ok(count)
    }

    #[tracing::instrument(
        name = "spider_classify_and_save_for_stage",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url, stage)
    )]
    async fn classify_and_save_for_stage(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        stage: &'static str,
    ) -> Result<(), SpiderServiceError> {
        if state.inference_sample.len() < self.config.min_inference_sample_urls {
            return Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: crawl_root_url.to_string(),
                stage,
                sample_size: state.inference_sample.len(),
                min_sample_size: self.config.min_inference_sample_urls,
            });
        }

        let classification = self
            .pattern_service
            .classify_and_save(
                listing_source_id,
                domain_id,
                crawl_root_url,
                state.pattern.as_regex(),
                &state.inference_sample,
            )
            .await;

        // A pending review is a wait, not a failure: the crawl keeps its
        // URLs under the pattern it has and the reviewer's decision
        // classifies them.
        if let Err(UrlPatternServiceError::PendingReview { review_id, .. }) = classification {
            info!(
                stage,
                review_id = %review_id,
                sample_size = state.inference_sample.len(),
                "Crawl waits for a URL pattern review"
            );
            state.pending_url_pattern_review = Some(review_id);
            return Ok(());
        }

        state.pattern = classification.map(|pattern| {
            pattern
                .map(ProductListingPattern::from)
                .unwrap_or(ProductListingPattern::Unknown)
        })?;

        if state.pattern.is_unknown() {
            warn!(stage, "Found no product URL pattern");
        }

        Ok(())
    }

    #[tracing::instrument(
        name = "spider_maybe_classify_at_threshold",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url, classify_threshold)
    )]
    async fn maybe_classify_at_threshold(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && state.total_crawled >= classify_threshold {
            debug!(
                url_count = state.inference_sample.len(),
                "Threshold reached, requesting product URL pattern"
            );

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "threshold",
            )
            .await?;

            state.classification_done = true;
            state.pattern_loaded_from_store = false;
        }

        Ok(())
    }

    async fn flush_batch_if_needed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), SpiderServiceError> {
        if state.classification_done && state.page_buffer.len() >= self.config.db_batch_size {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                )
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(name = "spider_log_progress", skip(self, state))]
    fn log_progress(&self, state: &CrawlRunState) {
        if state.total_crawled.is_multiple_of(1000) {
            info!(
                total_crawled = state.total_crawled,
                products_found = state.products_found,
                "Crawl progress"
            );
        }
    }

    #[tracing::instrument(
        name = "spider_classify_at_end_if_needed",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn classify_at_end_if_needed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && !state.page_buffer.is_empty() {
            info!(
                url_count = state.inference_sample.len(),
                "Threshold not reached, classifying collected URLs"
            );

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "end_of_crawl",
            )
            .await?;

            state.classification_done = true;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_reclassify_if_persisted_pattern_failed",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn reclassify_if_persisted_pattern_failed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        // Pages still buffered count too: a crawl smaller than one batch has
        // flushed nothing yet.
        if state.pattern_loaded_from_store
            && state.products_found == 0
            && count_products(&state.page_buffer, &state.pattern) == 0
            && !state.inference_sample.is_empty()
        {
            warn!("Persisted product URL pattern did not match crawl results, reclassifying");

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "refresh",
            )
            .await?;
        }

        Ok(())
    }

    async fn flush_remaining_pages(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), SpiderServiceError> {
        if !state.page_buffer.is_empty() {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                )
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_mark_as_crawled_best_effort",
        skip(self),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn mark_as_crawled_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) {
        if let Err(error) = self
            .pattern_service
            .mark_as_crawled(listing_source_id, domain_id)
            .await
        {
            warn!(error = ?error, "Failed to mark ListingSource as crawled");
        }
    }

    #[tracing::instrument(
        name = "spider_run_crawl",
        skip(self),
        fields(
            listing_source_id = %listing_source_id,
            domain_id = %domain_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run_crawl(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderServiceError::Discovery(SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::InvalidUrl,
                reason: "configured crawler domain URL is invalid",
                source: None,
            })
        })?;
        let crawl = self.spider.crawl(crawl_root_url).await?;
        let diagnostics_rx = crawl.diagnostics;
        let mut crawl_rx = crawl.pages;

        let initial_pattern = self
            .pattern_service
            .load_pattern_for_domain(listing_source_id, domain_id)
            .await?;
        let mut state = CrawlRunState::new(initial_pattern);

        if state.pattern_loaded_from_store {
            debug!("Loaded persisted product URL pattern");
        }

        while let Some(page) = crawl_rx.recv().await {
            if !is_same_or_www_host(&configured_root, page.url.as_url()) {
                debug!(url = %page.url, configured_root = %configured_root, "Ignoring discovered URL outside configured crawler domain");
                continue;
            }
            state.total_crawled += 1;

            if state.inference_sample.len() < self.config.max_sample_urls {
                state.inference_sample.push(page.url.to_string());
            }

            state.page_buffer.push(page.clone());

            self.maybe_classify_at_threshold(
                &mut state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                classify_threshold,
            )
            .await?;

            self.flush_batch_if_needed(&mut state, listing_source_id, domain_id)
                .await?;
            self.log_progress(&state);
        }

        // A filter task that died without reporting cannot vouch for a full crawl.
        let diagnostics = diagnostics_rx.await.unwrap_or_else(|_| CrawlDiagnostics {
            completion: CrawlCompletion::HardTimeoutTruncated,
            diagnostic_reason: Some("crawl_diagnostics_missing".to_string()),
            ..CrawlDiagnostics::default()
        });
        if let Some(error) = crawl_failure_error(crawl_root_url, state.total_crawled, &diagnostics)
        {
            return Err(error);
        }

        self.classify_at_end_if_needed(&mut state, listing_source_id, domain_id, crawl_root_url)
            .await?;
        self.reclassify_if_persisted_pattern_failed(
            &mut state,
            listing_source_id,
            domain_id,
            crawl_root_url,
        )
        .await?;
        self.flush_remaining_pages(&mut state, listing_source_id, domain_id)
            .await?;

        // URLs stay stored either way; only the recorded outcome differs.
        let completion = diagnostics.completion;
        if state.pending_url_pattern_review.is_none()
            && let Some(error) = no_product_urls_error(crawl_root_url, &state, completion)
        {
            return Err(error);
        }

        let product_pattern = state
            .pattern
            .as_regex()
            .map(|regex| regex.as_str().to_string());

        // `last_crawled` records the last full crawl; a truncated one keeps the
        // domain due so the scheduler can retry it sooner. A full crawl that
        // waits for a pattern review has discovered everything too: approval
        // classifies its stored URLs, so no second crawl from the root is due.
        if completion.is_full() {
            self.mark_as_crawled_best_effort(listing_source_id, domain_id, crawl_root_url)
                .await;
        }

        log_crawl_outcome(&state, &diagnostics, product_pattern.as_deref());

        Ok(SpiderRunResult {
            total_links: state.total_crawled,
            product_urls_count: state.products_found,
            product_pattern,
            completion,
            pending_url_pattern_review: state.pending_url_pattern_review,
        })
    }
}

#[async_trait::async_trait]
impl SpiderService for SpiderServiceImpl {
    #[tracing::instrument(
        name = "spider_run",
        skip(self),
        fields(
            listing_source_id = %listing_source_id,
            domain_id = %domain_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        debug!("Starting crawl");

        self.run_crawl(
            listing_source_id,
            domain_id,
            crawl_root_url,
            classify_threshold,
        )
        .await
    }
}

fn count_products(pages: &[CrawledPage], pattern: &ProductListingPattern) -> usize {
    pattern.as_regex().map_or(0, |regex| {
        pages
            .iter()
            .filter(|page| page.url.matches_pattern(regex))
            .count()
    })
}

/// Coverage losses on a crawl that delivered pages are only visible here.
fn log_crawl_outcome(
    state: &CrawlRunState,
    diagnostics: &CrawlDiagnostics,
    product_pattern: Option<&str>,
) {
    let completion = diagnostics.completion;
    info!(
        completion = %completion,
        at_budget = completion == CrawlCompletion::PageBudgetReached,
        diagnostic_reason = ?diagnostics.diagnostic_reason,
        truncated = !completion.is_full(),
        total_crawled = state.total_crawled,
        product_urls_count = state.products_found,
        product_pattern = ?product_pattern,
        product_pattern_known = product_pattern.is_some(),
        classification_done = state.classification_done,
        pages_received = diagnostics.pages_received,
        rejected_host = diagnostics.rejected_host,
        rejected_blacklist = diagnostics.rejected_blacklist,
        rejected_duplicate = diagnostics.rejected_duplicate,
        robots_policy = ?diagnostics.robots_policy,
        robots_crawl_delay_ms = ?diagnostics.robots_crawl_delay_ms,
        robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
        lagged = diagnostics.lagged,
        initial_anti_bot = ?diagnostics.initial_anti_bot,
        status_counts = ?diagnostics.status_counts,
        rate_limit = ?diagnostics.rate_limit,
        pending_url_pattern_review = ?state.pending_url_pattern_review.map(|id| id.to_string()),
        "{}",
        crawl_outcome_message(completion, state.pending_url_pattern_review.is_some())
    );
}

fn crawl_outcome_message(completion: CrawlCompletion, awaiting_review: bool) -> &'static str {
    match (completion, awaiting_review) {
        (_, true) => "Crawl waiting for URL pattern review, keeping discovered URLs",
        (CrawlCompletion::Completed, false) => "Crawl completed",
        (CrawlCompletion::PageBudgetReached, false) => "Crawl completed at page budget",
        (CrawlCompletion::HardTimeoutTruncated, false) => {
            "Crawl truncated by hard timeout, keeping discovered URLs"
        }
    }
}

fn classify_url(url: &str, product_pattern: &ProductListingPattern) -> UrlClass {
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
        let pattern = ProductListingPattern::Known(Regex::new(r"/product/").unwrap());

        let class = classify_url("https://example.com/product/42", &pattern);

        assert_eq!(class, UrlClass::ProductListing);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let pattern = ProductListingPattern::Unknown;

        let class = classify_url("https://example.com/impressum", &pattern);

        assert_eq!(class, UrlClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let pattern = ProductListingPattern::Unknown;

        let class = classify_url("https://example.com/collections/modern", &pattern);

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let pattern = ProductListingPattern::Unknown;

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
    use crate::spider::classification::url_metadata_repository::MockUrlMetadataRepository;
    use crate::spider::classification::url_pattern_service::MockUrlPatternService;
    use crate::spider::discovery::website_spider::{
        CrawlCompletion, CrawlDiagnostics, CrawlFailureKind, MockSpider, SpiderCrawl,
    };
    use regex::Regex;
    use tokio::sync::{mpsc, oneshot};

    fn setup_mock_url_repo(
        mock: &mut MockUrlMetadataRepository,
        call_count: usize,
        expected_domain_id: CrawlerDomainId,
    ) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _| Box::pin(async move { Ok(Vec::new()) }));
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, _crawl_root_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(mockall::predicate::always(), mockall::predicate::always())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
    }

    fn setup_mock_crawl<I, S>(mock: &mut MockSpider, crawl_root_url: &'static str, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        setup_mock_crawl_with_diagnostics(mock, crawl_root_url, paths, CrawlDiagnostics::default());
    }

    fn setup_mock_crawl_with_diagnostics<I, S>(
        mock: &mut MockSpider,
        crawl_root_url: &'static str,
        paths: I,
        diagnostics: CrawlDiagnostics,
    ) where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths: Vec<String> = paths.into_iter().map(Into::into).collect();
        mock.expect_crawl()
            .with(mockall::predicate::eq(crawl_root_url))
            .returning(move |_| {
                let paths = paths.clone();
                let diagnostics = diagnostics.clone();
                let (tx, rx) = mpsc::channel(25);
                tokio::spawn(async move {
                    for path in paths {
                        tx.send(CrawledPage {
                            url: CrawledUrl::new(
                                Url::parse(&format!("https://example.com{path}")).unwrap(),
                            ),
                        })
                        .await
                        .unwrap();
                    }
                });
                let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
                diagnostics_tx.send(diagnostics).unwrap();
                Box::pin(async {
                    Ok(SpiderCrawl {
                        pages: rx,
                        diagnostics: diagnostics_rx,
                    })
                })
            });
    }

    fn one_product_and_listing_pages() -> Vec<String> {
        let mut paths = vec!["/product/1".to_string()];
        paths.extend((1..20).map(|i| format!("/about/{i}")));
        paths
    }

    fn item_pages() -> Vec<String> {
        (1..=20).map(|i| format!("/item/{i}")).collect()
    }

    #[tokio::test]
    async fn should_run_spider_and_classify_urls() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
        assert_eq!(run_result.total_links, 20);
        assert_eq!(run_result.completion, CrawlCompletion::Completed);
    }

    fn product_pages_until_timeout() -> Vec<String> {
        (1..=25).map(|i| format!("/product/{i}")).collect()
    }

    #[tokio::test]
    async fn should_keep_urls_but_not_mark_crawled_when_crawl_hits_hard_timeout() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            product_pages_until_timeout(),
            CrawlDiagnostics {
                completion: CrawlCompletion::HardTimeoutTruncated,
                ..CrawlDiagnostics::default()
            },
        );
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, _, urls, _| urls.len() == 25)
            .returning(|_, _, _, _| Box::pin(async { Ok(Vec::new()) }));

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await
            .unwrap();

        assert_eq!(run_result.completion, CrawlCompletion::HardTimeoutTruncated);
        assert_eq!(run_result.total_links, 25);
        assert_eq!(run_result.product_urls_count, 25);
    }

    fn pending_review(
        listing_source_id: ListingSourceId,
        review_id: CrawlerReviewId,
    ) -> UrlPatternServiceError {
        UrlPatternServiceError::PendingReview {
            listing_source_id,
            review_id,
        }
    }

    #[tokio::test]
    async fn should_keep_urls_and_mark_crawled_when_pattern_review_is_pending_at_threshold() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let review_id = CrawlerReviewId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            product_pages_until_timeout(),
        );
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        // Asked once at the threshold; the pending review stops further asking.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, _, urls, classes| {
                urls.len() == 25
                    && classes
                        .iter()
                        .all(|class| *class != UrlClass::ProductListing)
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(Vec::new()) }));

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await
            .unwrap();

        assert_eq!(run_result.pending_url_pattern_review, Some(review_id));
        assert_eq!(run_result.total_links, 25);
        assert_eq!(run_result.product_urls_count, 0);
        assert_eq!(run_result.product_pattern, None);
        assert_eq!(run_result.completion, CrawlCompletion::Completed);
    }

    #[tokio::test]
    async fn should_keep_urls_but_not_mark_crawled_when_truncated_crawl_waits_for_pattern_review() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let review_id = CrawlerReviewId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            product_pages_until_timeout(),
            CrawlDiagnostics {
                completion: CrawlCompletion::HardTimeoutTruncated,
                ..CrawlDiagnostics::default()
            },
        );
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, _, urls, _| urls.len() == 25)
            .returning(|_, _, _, _| Box::pin(async { Ok(Vec::new()) }));

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await
            .unwrap();

        assert_eq!(run_result.pending_url_pattern_review, Some(review_id));
        assert_eq!(run_result.completion, CrawlCompletion::HardTimeoutTruncated);
    }

    #[tokio::test]
    async fn should_wait_for_review_when_refresh_of_persisted_pattern_opens_one() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let review_id = CrawlerReviewId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, _, crawl_pattern, _| {
                crawl_pattern.map(Regex::as_str) == Some(r"/product/")
            })
            .returning(move |_, _, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await
            .unwrap();

        assert_eq!(run_result.pending_url_pattern_review, Some(review_id));
        assert_eq!(run_result.product_urls_count, 0);
        assert_eq!(run_result.product_pattern.as_deref(), Some(r"/product/"));
    }

    #[tokio::test]
    async fn should_fail_with_no_product_urls_when_pattern_matches_no_crawled_page() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);
        // The crawled URLs are kept, only the outcome is a failure.
        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::NoProductUrls {
                crawl_root_url: url,
                total_links: 20,
                product_pattern,
                completion: CrawlCompletion::Completed,
            }) if url == crawl_root_url && product_pattern == "/product/"
        ));
    }

    #[tokio::test]
    async fn should_fail_with_no_url_pattern_when_classification_is_complete_without_one() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            item_pages(),
            CrawlDiagnostics {
                completion: CrawlCompletion::HardTimeoutTruncated,
                ..CrawlDiagnostics::default()
            },
        );
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        // A decided `NO_PATTERN` classification answers with no pattern.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_mark_as_crawled().times(0);
        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::NoUrlPattern {
                total_links: 20,
                completion: CrawlCompletion::HardTimeoutTruncated,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn should_report_page_budget_and_mark_crawled_when_crawl_is_bounded() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
            CrawlDiagnostics {
                completion: CrawlCompletion::PageBudgetReached,
                ..CrawlDiagnostics::default()
            },
        );
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await
            .unwrap();

        assert_eq!(run_result.completion, CrawlCompletion::PageBudgetReached);
    }

    #[test]
    fn should_never_log_hard_timeout_run_as_completed() {
        let message = crawl_outcome_message(CrawlCompletion::HardTimeoutTruncated, false);

        assert!(message.contains("truncated"), "{message}");
        assert!(!message.contains("completed"), "{message}");
        for completion in [
            CrawlCompletion::Completed,
            CrawlCompletion::PageBudgetReached,
            CrawlCompletion::HardTimeoutTruncated,
        ] {
            for awaiting_review in [false, true] {
                let message = crawl_outcome_message(completion, awaiting_review);
                assert!(!message.contains("successfully"), "{message}");
                assert_eq!(
                    message.contains("waiting for URL pattern review"),
                    awaiting_review,
                    "{message}"
                );
            }
        }
    }

    #[tokio::test]
    async fn should_classify_at_end_if_threshold_not_reached() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));

        // It should classify at the end because threshold is above the crawl size.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, _, crawl_pattern, _| crawl_pattern.is_none())
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
    }

    #[tokio::test]
    async fn should_reclassify_if_persisted_pattern_fails() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());

        // Persisted pattern expects /product/
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        // Reclassification gives the correct /item/ pattern
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, _, crawl_pattern, _| {
                crawl_pattern.map(Regex::as_str) == Some(r"/product/")
            })
            .returning(|_, _, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 20);
    }

    #[tokio::test]
    async fn should_keep_persisted_pattern_when_crawl_smaller_than_one_batch_matches_it() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        // Fewer pages than one database batch, as a crawl paced by a
        // robots.txt Crawl-delay delivers.
        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let run_result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 400)
            .await
            .unwrap();
        assert_eq!(run_result.product_urls_count, 20);
    }

    #[tokio::test]
    async fn should_return_empty_crawl_error_without_classifying_or_marking_crawled() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(crawl_root_url))
            .returning(|_| {
                let (_tx, rx) = mpsc::channel(10);
                let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
                diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
                Box::pin(async {
                    Ok(SpiderCrawl {
                        pages: rx,
                        diagnostics: diagnostics_rx,
                    })
                })
            });

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::CrawlFailed {
                crawl_root_url: url,
                kind: CrawlFailureKind::EmptyCrawl,
                total_links: 0,
                ..
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_tiny_crawl_error_for_one_url_without_classifying_or_marking_crawled() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/"]);

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::CrawlFailed {
                crawl_root_url: url,
                kind: CrawlFailureKind::TinyCrawl,
                total_links: 1,
                ..
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_insufficient_inference_sample_error_for_two_url_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            vec!["/collections", "/about"],
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "end_of_crawl",
                sample_size: 2,
                min_sample_size: 20,
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_diagnostic_crawl_failure_for_rate_limited_tiny_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            vec!["/"],
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::RateLimited),
                http_status: Some(429),
                final_url: Some("https://example.com/".to_string()),
                diagnostic_reason: Some("canonical_non_success_status".to_string()),
                ..CrawlDiagnostics::default()
            },
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::CrawlFailed {
                kind: CrawlFailureKind::RateLimited,
                total_links: 1,
                diagnostics,
                ..
            }) if diagnostics.http_status == Some(429)
        ));
    }

    #[tokio::test]
    async fn should_prefer_local_filter_diagnostic_over_empty_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            Vec::<String>::new(),
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::NoAcceptedPages),
                pages_received: 4,
                rejected_blacklist: 4,
                ..CrawlDiagnostics::default()
            },
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::CrawlFailed {
                kind: CrawlFailureKind::NoAcceptedPages,
                total_links: 0,
                diagnostics,
                ..
            }) if diagnostics.pages_received == 4
        ));
    }

    #[tokio::test]
    async fn should_keep_insufficient_inference_sample_when_diagnostic_has_multiple_urls() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            vec!["/collections", "/about"],
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::JavascriptRequired),
                diagnostic_reason: Some("few_links_and_app_shell_markers".to_string()),
                ..CrawlDiagnostics::default()
            },
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample { sample_size: 2, .. })
        ));
    }

    #[tokio::test]
    async fn should_return_insufficient_inference_sample_error_for_refresh_with_small_sample() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/item/1", "/item/2"]);

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "refresh",
                sample_size: 2,
                min_sample_size: 20,
            }) if url == crawl_root_url
        ));
    }
}

/// A crawl that delivered pages but no product URL is a failure, not a
/// completed crawl: the domain's pattern matched nothing, or its
/// classification is complete without one.
fn no_product_urls_error(
    crawl_root_url: &str,
    state: &CrawlRunState,
    completion: CrawlCompletion,
) -> Option<SpiderServiceError> {
    if state.products_found > 0 {
        return None;
    }
    Some(match state.pattern.as_regex() {
        Some(pattern) => SpiderServiceError::NoProductUrls {
            crawl_root_url: crawl_root_url.to_string(),
            total_links: state.total_crawled,
            product_pattern: pattern.as_str().to_string(),
            completion,
        },
        None => SpiderServiceError::NoUrlPattern {
            crawl_root_url: crawl_root_url.to_string(),
            total_links: state.total_crawled,
            completion,
        },
    })
}

/// A crawl with at most one usable page is a failure. A library or local
/// diagnostic explains it when present; otherwise the size alone does.
fn crawl_failure_error(
    crawl_root_url: &str,
    total_links: usize,
    diagnostics: &CrawlDiagnostics,
) -> Option<SpiderServiceError> {
    if total_links > 1 {
        return None;
    }
    let kind = diagnostics.failure_kind.unwrap_or(match total_links {
        0 => CrawlFailureKind::EmptyCrawl,
        _ => CrawlFailureKind::TinyCrawl,
    });
    Some(SpiderServiceError::CrawlFailed {
        crawl_root_url: crawl_root_url.to_string(),
        kind,
        total_links,
        diagnostics: Box::new(diagnostics.clone()),
    })
}
