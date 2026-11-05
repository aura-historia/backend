use crate::CrawlerReviewId;
use listing_source_core::ListingSourceId;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::network::policy::is_same_or_www_host;
use crate::spider::classification::url_classifier::UrlClassifier;
use crate::spider::classification::url_metadata::{CrawlerDisposition, UrlClass};
use crate::spider::classification::url_metadata_repository::{
    DiscoveredUrl, SpiderUrlRecord, UrlMetadataRepository,
};
use crate::spider::classification::url_pattern_service::{
    UrlPatternService, UrlPatternServiceError,
};
use crate::spider::crawl_frontier_repository::{CrawlFrontierRepository, KnownUrls};
use crate::spider::crawled_root::{CrawlOutcomeWrite, CrawledRoot};
use crate::spider::discovery::frontier::{CrawlDiscovery, CrawlKind, CrawlRequest, CrawlSeeds};
use crate::spider::discovery::page_status::RateLimitSignal;
use crate::spider::discovery::website_spider::{
    CrawlCompletion, CrawlDiagnostics, CrawlFailureKind, CrawlPace, CrawlStopReason, CrawledPage,
    Spider, SpiderDiscoveryError,
};
use crate::spider::service::crawl_run_state::{CrawlRunState, UrlPatternReviewWait};
use crate::spider::service::product_pattern::ProductListingPattern;
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::{ShopPlatform, canonical_product_url};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use url::Url;

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
    /// so the run is neither a success nor a failure. A rejection stores no
    /// decision for the crawl's re-reads to find, so the scheduler records the
    /// wait only while the review is still pending.
    pub pending_url_pattern_review: Option<CrawlerReviewId>,
    /// Product pages the ListingSource stores that an incremental crawl found linked
    /// and did not fetch again; never counted in `product_urls_count`.
    pub known_product_urls_skipped: usize,
}

/// A crawl that ended: the root it crawled and what the spider reported about it.
struct EndedCrawl {
    root: CrawledRoot,
    kind: CrawlKind,
    diagnostics: CrawlDiagnostics,
    /// `None` when the crawl could not tell what it left unvisited.
    discovery: Option<CrawlDiscovery>,
}

impl EndedCrawl {
    /// Product pages the ListingSource stores that the crawl found linked and, being incremental,
    /// did not fetch again. They are product pages the shop still links, so the crawl is not judged
    /// as if it had found no product page
    /// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
    fn skipped_known_products(&self) -> usize {
        match self.kind {
            CrawlKind::Incremental => self.diagnostics.known_skipped,
            CrawlKind::Full => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
    pub min_inference_sample_urls: usize,
    /// How crawled URLs are classified; the authoring mode selects it.
    pub url_classifier: UrlClassifier,
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

    /// The crawl delivered pages, but too few to classify their URL pattern; `diagnostics`
    /// carries what the crawl reported once it ended.
    #[error(
        "Cannot classify product URL pattern for crawl-root URL '{crawl_root_url}' at stage '{stage}' because the inference sample has {sample_size} URL(s), minimum is {min_sample_size}"
    )]
    InsufficientInferenceSample {
        crawl_root_url: String,
        stage: &'static str,
        sample_size: usize,
        min_sample_size: usize,
        diagnostics: Box<CrawlDiagnostics>,
    },

    /// The crawl delivered pages, but the ListingSource's product URL pattern
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

    /// The run failed after a shop's rate-limit answer had stopped its crawl.
    /// The scheduler honours the answer whatever the failure; `failure` keeps
    /// its own kind and evidence.
    #[error("spider run failed after a rate-limit answer ({}) stopped its crawl", rate_limit.status)]
    RateLimitedCrawl {
        rate_limit: RateLimitSignal,
        #[source]
        failure: Box<SpiderServiceError>,
    },

    /// The crawl delivered pages, but the ListingSource's URL classification is
    /// complete without a product pattern, so no page can become a product
    /// URL until an operator decides or resets the pattern.
    #[error(
        "Spider crawl for crawl-root URL '{crawl_root_url}' found {total_links} page(s) but the ListingSource has no product URL pattern"
    )]
    NoUrlPattern {
        crawl_root_url: String,
        total_links: usize,
        completion: CrawlCompletion,
    },

    /// The run found no product URL ([`Self::found_no_product_urls`]), but bot challenges were
    /// most of the pages its crawl received: the shop's bot protection, not a missing catalogue,
    /// kept the product pages from the crawl, so the run fails as bot protection
    /// (`docs/crawler/networking.md#bot-challenges`). `failure` is the failure it replaces.
    #[error(
        "spider run found no product URL after {bot_challenge_pages} of the {pages_received} page(s) its crawl received were bot challenges"
    )]
    BotChallengedCrawl {
        bot_challenge_pages: usize,
        pages_received: usize,
        #[source]
        failure: Box<SpiderServiceError>,
    },
}

impl SpiderServiceError {
    /// The failure itself, without the rate-limit answer that stopped the
    /// crawl before it.
    pub fn cause(&self) -> &SpiderServiceError {
        match self {
            SpiderServiceError::RateLimitedCrawl { failure, .. } => failure.cause(),
            failure => failure,
        }
    }

    /// The rate-limit answer that stopped the failed run's crawl, when one did.
    pub fn rate_limit(&self) -> Option<RateLimitSignal> {
        match self {
            SpiderServiceError::RateLimitedCrawl { rate_limit, .. } => Some(*rate_limit),
            _ => None,
        }
    }

    /// The failure with the diagnostics its crawl reported once it ended, also when bot
    /// challenges took the blame for it ([`Self::BotChallengedCrawl`]). A classification stage
    /// refuses a too small inference sample before it knows them: at the threshold, the crawl
    /// is still running.
    fn with_crawl_diagnostics(mut self, crawl_diagnostics: &CrawlDiagnostics) -> Self {
        self.attach_crawl_diagnostics(crawl_diagnostics);
        self
    }

    fn attach_crawl_diagnostics(&mut self, crawl_diagnostics: &CrawlDiagnostics) {
        match self {
            SpiderServiceError::InsufficientInferenceSample { diagnostics, .. } => {
                **diagnostics = crawl_diagnostics.clone();
            }
            SpiderServiceError::BotChallengedCrawl { failure, .. } => {
                failure.attach_crawl_diagnostics(crawl_diagnostics);
            }
            _ => {}
        }
    }

    /// The run delivered pages, but none of them can become a product URL: the inference sample was
    /// too small, the ListingSource's pattern matched none of them, or its classification is
    /// complete without a pattern. Only a site or configuration change resolves it, so the
    /// scheduler retries it on the recoverable cooldown and escalates it to an operator.
    pub fn found_no_product_urls(&self) -> bool {
        matches!(
            self.cause(),
            SpiderServiceError::InsufficientInferenceSample { .. }
                | SpiderServiceError::NoProductUrls { .. }
                | SpiderServiceError::NoUrlPattern { .. }
        )
    }
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
            min_inference_sample_urls: 20,
            url_classifier: UrlClassifier::default(),
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SpiderService: Send + Sync {
    /// Crawls the ListingSource as `kind` asks: a full crawl records the last full crawl when it
    /// reached everything, an incremental one never does.
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        _crawl_root_url: &str,
        classify_threshold: usize,
        pace: CrawlPace,
        kind: CrawlKind,
    ) -> Result<SpiderRunResult, SpiderServiceError>;
}

pub struct SpiderServiceImpl {
    config: SpiderServiceConfig,
    spider: Box<dyn Spider>,
    pattern_service: Box<dyn UrlPatternService>,
    url_metadata_repository: Arc<dyn UrlMetadataRepository>,
    frontier_repository: Arc<dyn CrawlFrontierRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        config: SpiderServiceConfig,
        spider: Box<dyn Spider>,
        pattern_service: Box<dyn UrlPatternService>,
        url_metadata_repository: Arc<dyn UrlMetadataRepository>,
        frontier_repository: Arc<dyn CrawlFrontierRepository>,
    ) -> Self {
        Self {
            config,
            spider,
            pattern_service,
            url_metadata_repository,
            frontier_repository,
        }
    }

    /// What the crawl starts from besides its root: the frontier the previous crawl left, and
    /// the known product URLs an incremental crawl skips.
    async fn crawl_seeds(
        &self,
        listing_source_id: &ListingSourceId,
        kind: CrawlKind,
    ) -> Result<CrawlSeeds, SpiderServiceError> {
        let frontier = self
            .frontier_repository
            .load_frontier(listing_source_id)
            .await?;
        let known = match kind {
            CrawlKind::Full => KnownUrls::default(),
            CrawlKind::Incremental => {
                self.frontier_repository
                    .known_urls(listing_source_id)
                    .await?
            }
        };
        Ok(CrawlSeeds {
            frontier,
            known: known.products,
            retired: known.retired,
        })
    }

    /// Keeps what the crawl of `root` left unvisited for the ListingSource's next crawl. A failed
    /// write leaves the previous frontier, which the next crawl revisits at worst; a ListingSource
    /// that moved to another root keeps the empty frontier the move left.
    async fn replace_frontier_best_effort(
        &self,
        root: &CrawledRoot,
        discovery: Option<&CrawlDiscovery>,
    ) {
        let Some(discovery) = discovery else {
            warn!("Spider crawl reported no frontier; keeping the previous one");
            return;
        };
        match self
            .frontier_repository
            .replace_frontier(root, &discovery.frontier)
            .await
        {
            Ok(CrawlOutcomeWrite::Recorded) => {}
            Ok(CrawlOutcomeWrite::RootMoved) => debug!(
                frontier_urls = discovery.frontier.len(),
                "The ListingSource moved to another crawl root during the crawl; its frontier is not kept"
            ),
            Err(error) => warn!(error = ?error, "Failed to persist the crawl frontier"),
        }
    }

    /// Stores `pages` classified by `pattern`, each product page under its canonical URL
    /// ([`canonical_product_url`]). This is the one path every discovered URL is stored through,
    /// so the canonical rules cover every page a crawl stores.
    async fn persist_url_metadata_batch(
        &self,
        listing_source_id: &ListingSourceId,
        pages: &[CrawledPage],
        pattern: &ProductListingPattern,
        shop_platform: Option<ShopPlatform>,
    ) -> Result<Vec<SpiderUrlRecord>, SpiderServiceError> {
        if pages.is_empty() {
            return Ok(Vec::new());
        }

        let links = pages
            .iter()
            .map(|page| self.discovered_url(page, pattern, shop_platform))
            .collect::<Vec<_>>();

        self.url_metadata_repository
            .upsert_links_batch(listing_source_id, &links)
            .await
            .map_err(|error| SpiderServiceError::UrlMetadata(Box::new(error)))
    }

    fn discovered_url(
        &self,
        page: &CrawledPage,
        pattern: &ProductListingPattern,
        shop_platform: Option<ShopPlatform>,
    ) -> DiscoveredUrl {
        let url_class = self
            .config
            .url_classifier
            .classify(page.url.as_url().as_str(), pattern.as_regex());
        let canonical = pattern
            .as_regex()
            .filter(|_| url_class == UrlClass::ProductListing)
            .and_then(|regex| {
                canonical_product_url(&page.url, &page.identity, shop_platform, regex)
            })
            .map(|canonical| canonical.as_url().clone());
        DiscoveredUrl {
            url: page.url.as_url().clone(),
            url_class,
            canonical,
        }
    }

    /// Stores the buffered pages under the crawl's pattern, counts its product URLs among them
    /// and empties the buffer. A crawl that waits on a URL-pattern review keeps the pages it
    /// stored for the review's decision.
    async fn store_buffered_pages(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), SpiderServiceError> {
        record_pattern_matches(
            &state.page_buffer,
            &state.pattern,
            &mut state.pattern_matched_pages,
        );
        let stored = self
            .persist_url_metadata_batch(
                listing_source_id,
                &state.page_buffer,
                &state.pattern,
                state.shop_platform,
            )
            .await?;
        record_stored_products(&stored, &mut state.stored_product_pages);
        match &mut state.url_pattern_review_wait {
            Some(wait) => wait.stored_pages.append(&mut state.page_buffer),
            None => state.page_buffer.clear(),
        }
        Ok(())
    }

    /// Re-reads the ListingSource's pattern state while the crawl waits on a URL-pattern review.
    /// When the review was decided while the crawl ran, the crawl stores the pages it stored while
    /// waiting again, classified with the decision, and stops waiting, so no URL it stored after
    /// the decision stays unclassified and its outcome is the completion the decision gives.
    #[tracing::instrument(
        name = "spider_classify_with_decision_made_while_waiting",
        skip(self, state),
        fields(listing_source_id = %listing_source_id)
    )]
    async fn classify_with_decision_made_while_waiting(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), SpiderServiceError> {
        let Some(review_id) = state.pending_url_pattern_review() else {
            return Ok(());
        };
        let Some(decision) = self
            .pattern_service
            .decision_stored_during_crawl(listing_source_id, state.pattern.as_regex())
            .await?
        else {
            return Ok(());
        };
        let stored_pages = state.end_url_pattern_review_wait(ProductListingPattern::from(decision));
        info!(
            review_id = %review_id,
            stored_urls = stored_pages.len(),
            product_pattern = ?state.pattern.as_regex().map(regex::Regex::as_str),
            "URL pattern review was decided during the crawl, classifying its stored URLs with the decision"
        );
        for pages in stored_pages.chunks(self.config.db_batch_size.max(1)) {
            record_pattern_matches(pages, &state.pattern, &mut state.pattern_matched_pages);
            let stored = self
                .persist_url_metadata_batch(
                    listing_source_id,
                    pages,
                    &state.pattern,
                    state.shop_platform,
                )
                .await?;
            record_stored_products(&stored, &mut state.stored_product_pages);
        }
        Ok(())
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
        crawl_root_url: &str,
        stage: &'static str,
    ) -> Result<(), SpiderServiceError> {
        if state.inference_sample.len() < self.config.min_inference_sample_urls {
            return Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: crawl_root_url.to_string(),
                stage,
                sample_size: state.inference_sample.len(),
                min_sample_size: self.config.min_inference_sample_urls,
                // Known once the crawl has ended; the run attaches them (`with_crawl_diagnostics`).
                diagnostics: Box::default(),
            });
        }

        let classification = self
            .pattern_service
            .classify_and_save(
                listing_source_id,
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
            state.url_pattern_review_wait = Some(UrlPatternReviewWait {
                review_id,
                stored_pages: Vec::new(),
            });
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
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<(), SpiderServiceError> {
        // A sample that is mostly challenges is no evidence of the shop's URLs: the crawl waits,
        // asks again with every page it receives and leaves the verdict to the end of the crawl
        // ([`Self::classify_at_end_if_needed`]), which knows its diagnostics.
        if !state.classification_done
            && state.total_crawled >= classify_threshold
            && !state.mostly_bot_challenges()
        {
            debug!(
                url_count = state.inference_sample.len(),
                "Threshold reached, requesting product URL pattern"
            );

            self.classify_and_save_for_stage(state, listing_source_id, crawl_root_url, "threshold")
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
    ) -> Result<(), SpiderServiceError> {
        if state.classification_done && state.page_buffer.len() >= self.config.db_batch_size {
            self.store_buffered_pages(state, listing_source_id).await?;
            self.classify_with_decision_made_while_waiting(state, listing_source_id)
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(name = "spider_log_progress", skip(self, state))]
    fn log_progress(&self, state: &CrawlRunState) {
        if state.total_crawled.is_multiple_of(1000) {
            info!(
                total_crawled = state.total_crawled,
                products_found = state.products_found(),
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
        crawl_root_url: &str,
        diagnostics: &CrawlDiagnostics,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && !state.page_buffer.is_empty() {
            // A first classification of pages that were mostly bot challenges neither opens a
            // review nor runs inference on them; the run fails as bot protection
            // ([`blame_bot_challenges`]), as one with a stored pattern does.
            if mostly_bot_challenges(diagnostics) {
                return Err(SpiderServiceError::InsufficientInferenceSample {
                    crawl_root_url: crawl_root_url.to_string(),
                    stage: "end_of_crawl",
                    // Nothing is flushed before the first classification, so the buffer
                    // still holds every page of the sample, in sample order.
                    sample_size: state
                        .page_buffer
                        .iter()
                        .take(state.inference_sample.len())
                        .filter(|page| !page.bot_challenge)
                        .count(),
                    min_sample_size: self.config.min_inference_sample_urls,
                    diagnostics: Box::default(),
                });
            }
            info!(
                url_count = state.inference_sample.len(),
                "Threshold not reached, classifying collected URLs"
            );

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
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
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        // Pages still buffered count too: a crawl smaller than one batch has
        // flushed nothing yet.
        if state.pattern_loaded_from_store
            && !state.pattern_matched_any()
            && !has_pattern_match(&state.page_buffer, &state.pattern)
            && !state.inference_sample.is_empty()
        {
            warn!("Persisted product URL pattern did not match crawl results, reclassifying");

            self.classify_and_save_for_stage(state, listing_source_id, crawl_root_url, "refresh")
                .await?;
        }

        Ok(())
    }

    async fn flush_remaining_pages(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
    ) -> Result<(), SpiderServiceError> {
        if !state.page_buffer.is_empty() {
            self.store_buffered_pages(state, listing_source_id).await?;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_mark_as_crawled_best_effort",
        skip(self),
        fields(listing_source_id = %root.listing_source_id, crawl_root_url = %root.crawl_root_url)
    )]
    async fn mark_as_crawled_best_effort(&self, root: &CrawledRoot) {
        match self.pattern_service.mark_as_crawled(root).await {
            Ok(CrawlOutcomeWrite::Recorded) => {}
            Ok(CrawlOutcomeWrite::RootMoved) => debug!(
                "The ListingSource moved to another crawl root during the crawl; its full crawl is not recorded"
            ),
            Err(error) => warn!(error = ?error, "Failed to mark ListingSource as crawled"),
        }
    }

    /// Loads the ListingSource's pattern and consumes every page the crawl delivers,
    /// classifying at the threshold and flushing full batches.
    async fn consume_pages(
        &self,
        mut crawl_rx: mpsc::Receiver<CrawledPage>,
        configured_root: &Url,
        listing_source_id: &ListingSourceId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<CrawlRunState, SpiderServiceError> {
        let initial_pattern = self.pattern_service.load_pattern(listing_source_id).await?;
        let mut state = CrawlRunState::new(initial_pattern);

        if state.pattern_loaded_from_store {
            debug!("Loaded persisted product URL pattern");
        }

        while let Some(page) = crawl_rx.recv().await {
            if !is_same_or_www_host(configured_root, page.url.as_url()) {
                debug!(url = %page.url, configured_root = %configured_root, "Ignoring discovered URL outside the configured crawl host");
                continue;
            }
            state.total_crawled += 1;
            state.challenged_pages += usize::from(page.bot_challenge);
            state.shop_platform = state.shop_platform.or(page.identity.platform);

            if state.inference_sample.len() < self.config.max_sample_urls {
                state.inference_sample.push(page.url.to_string());
            }

            state.page_buffer.push(page.clone());

            self.maybe_classify_at_threshold(
                &mut state,
                listing_source_id,
                crawl_root_url,
                classify_threshold,
            )
            .await?;

            self.flush_batch_if_needed(&mut state, listing_source_id)
                .await?;
            self.log_progress(&state);
        }

        Ok(state)
    }

    #[tracing::instrument(
        name = "spider_run_crawl",
        skip(self),
        fields(
            listing_source_id = %listing_source_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run_crawl(
        &self,
        listing_source_id: &ListingSourceId,
        crawl_root_url: &str,
        classify_threshold: usize,
        pace: CrawlPace,
        kind: CrawlKind,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderServiceError::Discovery(SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::InvalidUrl,
                reason: "configured crawl root URL is invalid",
                source: None,
                root_reroot: None,
            })
        })?;
        let root = CrawledRoot::new(*listing_source_id, crawl_root_url);
        let seeds = self.crawl_seeds(listing_source_id, kind).await?;
        let crawl = self
            .spider
            .crawl(CrawlRequest {
                crawl_root_url: crawl_root_url.to_string(),
                pace,
                seeds,
            })
            .await?;
        let consumed = self
            .consume_pages(
                crawl.pages,
                &configured_root,
                listing_source_id,
                crawl_root_url,
                classify_threshold,
            )
            .await;

        // A run that ends early has dropped the page channel, which stops
        // the library crawl; its diagnostics arrive only once that crawl has
        // ended, so the run, and the work lock its caller holds, outlive
        // every request of the crawl. A filter task that died without
        // reporting cannot vouch for a full crawl.
        let diagnostics = crawl
            .diagnostics
            .await
            .unwrap_or_else(|_| CrawlDiagnostics {
                completion: CrawlCompletion::HardTimeoutTruncated,
                diagnostic_reason: Some("crawl_diagnostics_missing".to_string()),
                ..CrawlDiagnostics::default()
            });
        let ended = EndedCrawl {
            root,
            kind,
            discovery: crawl.discovery.await.ok(),
            diagnostics,
        };
        let rate_limit = ended.diagnostics.completion.rate_limit();
        self.finish_crawl(consumed, &ended, listing_source_id, crawl_root_url)
            .await
            .map_err(|error| error.with_crawl_diagnostics(&ended.diagnostics))
            .map_err(|error| match rate_limit {
                Some(rate_limit) => SpiderServiceError::RateLimitedCrawl {
                    rate_limit,
                    failure: Box::new(error),
                },
                None => error,
            })
    }

    /// Classifies, stores and judges what the ended crawl delivered: the run state of a crawl
    /// whose outcome is a success or a review wait, or the failure it ended with.
    async fn judge_delivered_pages(
        &self,
        consumed: Result<CrawlRunState, SpiderServiceError>,
        ended: &EndedCrawl,
        listing_source_id: &ListingSourceId,
        crawl_root_url: &str,
    ) -> Result<CrawlRunState, SpiderServiceError> {
        let diagnostics = &ended.diagnostics;
        let skipped_known_products = ended.skipped_known_products();
        let mut state =
            consumed.inspect_err(|error| log_crawl_stopped_early(diagnostics, error))?;
        if let Some(error) = crawl_failure_error(
            crawl_root_url,
            state.total_crawled,
            skipped_known_products,
            diagnostics,
        ) {
            log_crawl_stopped_early(diagnostics, &error);
            return Err(error);
        }

        self.classify_at_end_if_needed(&mut state, listing_source_id, crawl_root_url, diagnostics)
            .await?;
        // A stored pattern that matched none of the pages an incremental crawl
        // fetched still takes the stored product pages the crawl skipped.
        let judged_on_fetched_pages =
            skipped_known_products == 0 || !state.pattern_loaded_from_store;
        // Pages that were mostly bot challenges are no evidence against a stored pattern either:
        // the run fails as bot protection ([`blame_bot_challenges`]) instead of opening a
        // URL-pattern review or running inference on challenges.
        if judged_on_fetched_pages && !mostly_bot_challenges(diagnostics) {
            self.reclassify_if_persisted_pattern_failed(
                &mut state,
                listing_source_id,
                crawl_root_url,
            )
            .await?;
        }
        self.flush_remaining_pages(&mut state, listing_source_id)
            .await?;
        self.classify_with_decision_made_while_waiting(&mut state, listing_source_id)
            .await?;

        // URLs stay stored either way; only the recorded outcome differs.
        if state.pending_url_pattern_review().is_none()
            && judged_on_fetched_pages
            && let Some(error) =
                no_product_urls_error(crawl_root_url, &state, diagnostics.completion)
        {
            return Err(error);
        }
        Ok(state)
    }

    /// Judges what the ended crawl delivered and keeps what it left unvisited for the next
    /// crawl. A run that failed only because its pages yield no product URL
    /// ([`SpiderServiceError::found_no_product_urls`]) keeps it too, so the next crawl reaches
    /// further, and fails as bot protection when those pages were mostly bot challenges
    /// ([`blame_bot_challenges`]); any other failed run keeps the previous frontier
    /// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
    async fn finish_crawl(
        &self,
        consumed: Result<CrawlRunState, SpiderServiceError>,
        ended: &EndedCrawl,
        listing_source_id: &ListingSourceId,
        crawl_root_url: &str,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let EndedCrawl {
            root,
            kind,
            discovery,
            diagnostics,
        } = ended;
        let (kind, discovery) = (*kind, discovery.as_ref());
        let judged = self
            .judge_delivered_pages(consumed, ended, listing_source_id, crawl_root_url)
            .await;
        let keeps_its_frontier = match &judged {
            Ok(_) => true,
            Err(error) => error.found_no_product_urls(),
        };
        if keeps_its_frontier {
            self.replace_frontier_best_effort(root, discovery).await;
        }
        if let Err(error) = &judged
            && keeps_its_frontier
        {
            log_failed_run_frontier(error, diagnostics, discovery);
        }
        let state = judged.map_err(|failure| blame_bot_challenges(failure, diagnostics))?;
        let completion = diagnostics.completion;

        let product_pattern = state
            .pattern
            .as_regex()
            .map(|regex| regex.as_str().to_string());

        // `last_crawled` records the last full crawl; a truncated one keeps the
        // ListingSource due so the scheduler can retry it sooner. A full crawl that
        // waits for a pattern review has discovered everything too: approval
        // classifies its stored URLs, so no second crawl from the root is due.
        // An incremental crawl skipped known product pages, so it is never one.
        if completion.is_full() && kind == CrawlKind::Full {
            self.mark_as_crawled_best_effort(root).await;
        }

        log_crawl_outcome(
            &state,
            diagnostics,
            product_pattern.as_deref(),
            kind,
            discovery,
        );

        Ok(SpiderRunResult {
            total_links: state.total_crawled,
            product_urls_count: state.products_found(),
            product_pattern,
            completion,
            pending_url_pattern_review: state.pending_url_pattern_review(),
            known_product_urls_skipped: ended.skipped_known_products(),
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
            crawl_root_url = %crawl_root_url,
            classify_threshold,
            pace = ?pace,
            kind = %kind
        )
    )]
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        crawl_root_url: &str,
        classify_threshold: usize,
        pace: CrawlPace,
        kind: CrawlKind,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        debug!("Starting crawl");

        self.run_crawl(
            listing_source_id,
            crawl_root_url,
            classify_threshold,
            pace,
            kind,
        )
        .await
    }
}

/// Adds the pages among `pages` the product pattern matches to `matched` by their page spelling,
/// so a page a shop links with and without its trailing slash, in one batch or two, counts once.
fn record_pattern_matches(
    pages: &[CrawledPage],
    pattern: &ProductListingPattern,
    matched: &mut HashSet<Url>,
) {
    let Some(regex) = pattern.as_regex() else {
        return;
    };
    matched.extend(
        pages
            .iter()
            .filter(|page| page.url.matches_pattern(regex))
            .map(|page| page.url.page_spelling()),
    );
}

/// Adds the rows storage keeps as product URLs to `products` by their page spelling. Storage decides
/// the class, so a page the pattern matches but an operator rejected is no product URL.
/// Records the product pages a batch stored. A row retired as a duplicate keeps its class but is
/// no product page of its own: its product is stored under its canonical URL.
fn record_stored_products(stored: &[SpiderUrlRecord], products: &mut HashSet<Url>) {
    products.extend(
        stored
            .iter()
            .filter(|record| {
                record.url_class == UrlClass::ProductListing
                    && record.disposition != CrawlerDisposition::Duplicate
            })
            .map(|record| CrawledUrl::new(record.url.clone()).page_spelling()),
    );
}

fn has_pattern_match(pages: &[CrawledPage], pattern: &ProductListingPattern) -> bool {
    pattern
        .as_regex()
        .is_some_and(|regex| pages.iter().any(|page| page.url.matches_pattern(regex)))
}

/// Coverage losses on a crawl that delivered pages are only visible here.
fn log_crawl_outcome(
    state: &CrawlRunState,
    diagnostics: &CrawlDiagnostics,
    product_pattern: Option<&str>,
    kind: CrawlKind,
    discovery: Option<&CrawlDiscovery>,
) {
    let completion = diagnostics.completion;
    info!(
        completion = %completion,
        kind = %kind,
        crawl_budget_ms = diagnostics.crawl_budget_ms,
        seeds = diagnostics.seeds,
        known_skipped = diagnostics.known_skipped,
        known_fetches_saved = diagnostics.known_fetches_saved,
        root_reroot = ?diagnostics.root_reroot,
        frontier_left = ?discovery.map(|discovery| discovery.frontier.len()),
        at_budget = completion == CrawlCompletion::PageBudgetReached,
        diagnostic_reason = ?diagnostics.diagnostic_reason,
        truncated = !completion.is_full(),
        total_crawled = state.total_crawled,
        product_urls_count = state.products_found(),
        product_pattern = ?product_pattern,
        product_pattern_known = product_pattern.is_some(),
        classification_done = state.classification_done,
        pages_received = diagnostics.pages_received,
        rejected_host = diagnostics.rejected_host,
        rejected_blacklist = diagnostics.rejected_blacklist,
        rejected_duplicate = diagnostics.rejected_duplicate,
        robots_policy = ?diagnostics.robots_policy,
        robots_crawl_delay_ms = ?diagnostics.robots_crawl_delay_ms,
        request_delay_ms = diagnostics.request_delay_ms,
        robots_rules_failing_closed = diagnostics.robots_rules_failing_closed,
        lagged = diagnostics.lagged,
        initial_anti_bot = ?diagnostics.initial_anti_bot,
        status_counts = ?diagnostics.status_counts,
        bot_challenge_pages = diagnostics.bot_challenge_pages,
        rate_limit = ?diagnostics.rate_limit,
        pending_url_pattern_review = ?state.pending_url_pattern_review().map(|id| id.to_string()),
        stopped_early = ?diagnostics.stopped_early.map(CrawlStopReason::as_str),
        "{}",
        crawl_outcome_message(
            completion,
            diagnostics.stopped_early,
            state.pending_url_pattern_review().is_some()
        )
    );
}

/// The frontier a run that found no product URL left for the next crawl: `frontier_left` is
/// absent when the crawl could not tell it, and a failed write is logged on its own. The
/// scheduler logs the failure itself.
fn log_failed_run_frontier(
    error: &SpiderServiceError,
    diagnostics: &CrawlDiagnostics,
    discovery: Option<&CrawlDiscovery>,
) {
    info!(
        error = %error,
        completion = %diagnostics.completion,
        crawl_budget_ms = diagnostics.crawl_budget_ms,
        seeds = diagnostics.seeds,
        frontier_left = ?discovery.map(|discovery| discovery.frontier.len()),
        root_reroot = ?diagnostics.root_reroot,
        pages_received = diagnostics.pages_received,
        bot_challenge_pages = diagnostics.bot_challenge_pages,
        "Spider run found no product URL among the pages it delivered"
    );
}

/// The completion log of a failed run whose library crawl discovery stopped
/// before Spider ended it.
fn log_crawl_stopped_early(diagnostics: &CrawlDiagnostics, error: &SpiderServiceError) {
    let Some(stop_reason) = diagnostics.stopped_early else {
        return;
    };
    info!(
        stop_reason = %stop_reason,
        error = %error,
        completion = %diagnostics.completion,
        pages_received = diagnostics.pages_received,
        accepted = diagnostics.accepted,
        "Spider run failed after the crawl was stopped early ({stop_reason})"
    );
}

fn crawl_outcome_message(
    completion: CrawlCompletion,
    stopped_early: Option<CrawlStopReason>,
    awaiting_review: bool,
) -> &'static str {
    match (completion, awaiting_review) {
        (_, true) => "Crawl waiting for URL pattern review, keeping discovered URLs",
        (CrawlCompletion::Completed, false) => "Crawl completed",
        (CrawlCompletion::PageBudgetReached, false) => "Crawl completed at page budget",
        (CrawlCompletion::RateLimited(_), false) => {
            "Crawl rate-limited: stopped early on a rate-limit answer, keeping discovered URLs"
        }
        (CrawlCompletion::HardTimeoutTruncated, false) => match stopped_early {
            Some(CrawlStopReason::RunEnded) => {
                "Crawl truncated: stopped early because the run ended, keeping discovered URLs"
            }
            // A rate-limit stop ends the crawl as rate-limited instead.
            Some(CrawlStopReason::RateLimited) | None => {
                "Crawl truncated by hard timeout, keeping discovered URLs"
            }
        },
    }
}

/// A run that found no product URL ([`SpiderServiceError::found_no_product_urls`]) among pages
/// that were mostly bot challenges met the shop's bot protection, not a shop without a catalogue,
/// and fails as such; below that share it keeps its own failure
/// (`docs/crawler/networking.md#bot-challenges`).
fn blame_bot_challenges(
    failure: SpiderServiceError,
    diagnostics: &CrawlDiagnostics,
) -> SpiderServiceError {
    if !failure.found_no_product_urls() || !mostly_bot_challenges(diagnostics) {
        return failure;
    }
    SpiderServiceError::BotChallengedCrawl {
        bot_challenge_pages: diagnostics.bot_challenge_pages,
        pages_received: diagnostics.pages_received,
        failure: Box::new(failure),
    }
}

/// Whether bot challenges were more than half of the pages the crawl received.
fn mostly_bot_challenges(diagnostics: &CrawlDiagnostics) -> bool {
    let other_pages = diagnostics
        .pages_received
        .saturating_sub(diagnostics.bot_challenge_pages);
    diagnostics.bot_challenge_pages > other_pages
}

#[cfg(test)]
mod tests {
    use crate::spider::classification::url_metadata::UrlClass;

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
    use crate::spider::classification::url_classifier::UrlClassifier;
    use crate::spider::classification::url_metadata::CrawlerDisposition;
    use crate::spider::classification::url_metadata_repository::MockUrlMetadataRepository;
    use crate::spider::classification::url_pattern_service::{
        DecidedUrlPattern, MockUrlPatternService,
    };
    use crate::spider::crawl_frontier_repository::MockCrawlFrontierRepository;
    use crate::spider::discovery::page_status::StatusCounts;
    use crate::spider::discovery::website_spider::{
        CrawlCompletion, CrawlDiagnostics, CrawlFailureKind, MockSpider, SpiderCrawl,
    };
    use crate::spider::utils::url::CrawledUrl;
    use crate::spider::utils::url::canonical::PageIdentityEvidence;
    use regex::Regex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::{mpsc, oneshot};

    /// The crawl a ListingSource without a frontier starts with: its root alone.
    fn first_full_crawl(crawl_root_url: &str) -> CrawlRequest {
        CrawlRequest::from_root(crawl_root_url, CrawlPace::default())
    }

    /// A crawl that cannot tell what it left unvisited.
    fn discovery_unknown() -> oneshot::Receiver<CrawlDiscovery> {
        oneshot::channel().1
    }

    /// A ListingSource with no frontier, whose crawl's frontier is not checked.
    fn no_frontier() -> Arc<MockCrawlFrontierRepository> {
        let mut repository = MockCrawlFrontierRepository::new();
        repository
            .expect_load_frontier()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        repository
            .expect_known_urls()
            .returning(|_| Box::pin(async { Ok(KnownUrls::default()) }));
        repository
            .expect_replace_frontier()
            .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        Arc::new(repository)
    }

    /// The rows the URL repository answers a batch with when it stores each URL as it was given.
    fn stored_as_given(
        listing_source_id: &ListingSourceId,
        links: &[DiscoveredUrl],
    ) -> Vec<SpiderUrlRecord> {
        let now = time::OffsetDateTime::now_utc();
        links
            .iter()
            .map(|link| SpiderUrlRecord {
                listing_source_id: *listing_source_id,
                url: link.canonical.clone().unwrap_or_else(|| link.url.clone()),
                url_class: link.url_class,
                disposition: CrawlerDisposition::Active,
                last_scraped_hash: None,
                last_scraped: None,
                created: now,
                updated: now,
            })
            .collect()
    }

    fn setup_mock_url_repo(
        mock: &mut MockUrlMetadataRepository,
        call_count: usize,
        expected_listing_source_id: ListingSourceId,
    ) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .withf(move |listing_source_id, _| *listing_source_id == expected_listing_source_id)
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, _crawl_root_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(mockall::predicate::always())
            .times(1)
            .returning(|_| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
    }

    /// The page a mock crawl delivers for `path`; a path under `/challenge` is a bot challenge
    /// the shop answered instead of the page.
    fn crawled_page(path: &str) -> CrawledPage {
        CrawledPage::new(CrawledUrl::new(
            Url::parse(&format!("https://example.com{path}")).unwrap(),
        ))
        .challenged(path.starts_with("/challenge"))
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
            .with(mockall::predicate::eq(first_full_crawl(crawl_root_url)))
            .returning(move |_| {
                let paths = paths.clone();
                let diagnostics = diagnostics.clone();
                let (tx, rx) = mpsc::channel(25);
                tokio::spawn(async move {
                    for path in paths {
                        tx.send(crawled_page(&path)).await.unwrap();
                    }
                });
                let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
                diagnostics_tx.send(diagnostics).unwrap();
                Box::pin(async {
                    Ok(SpiderCrawl {
                        pages: rx,
                        diagnostics: diagnostics_rx,
                        discovery: discovery_unknown(),
                    })
                })
            });
    }

    /// A crawl of `request` that delivers `paths` and reports `discovery` once it ended.
    fn setup_mock_seeded_crawl(
        mock: &mut MockSpider,
        request: CrawlRequest,
        paths: Vec<String>,
        discovery: CrawlDiscovery,
    ) {
        setup_mock_seeded_crawl_with_diagnostics(
            mock,
            request,
            paths,
            discovery,
            CrawlDiagnostics::default(),
        );
    }

    /// A crawl of `request` that delivers `paths` and reports `diagnostics` and `discovery`
    /// once it ended.
    fn setup_mock_seeded_crawl_with_diagnostics(
        mock: &mut MockSpider,
        request: CrawlRequest,
        paths: Vec<String>,
        discovery: CrawlDiscovery,
        diagnostics: CrawlDiagnostics,
    ) {
        mock.expect_crawl()
            .with(mockall::predicate::eq(request))
            .times(1)
            .return_once(move |_| {
                let (tx, rx) = mpsc::channel(paths.len().max(1));
                for path in paths {
                    tx.try_send(crawled_page(&path)).unwrap();
                }
                let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
                diagnostics_tx.send(diagnostics).unwrap();
                let (discovery_tx, discovery_rx) = oneshot::channel();
                discovery_tx.send(discovery).unwrap();
                Box::pin(async {
                    Ok(SpiderCrawl {
                        pages: rx,
                        diagnostics: diagnostics_rx,
                        discovery: discovery_rx,
                    })
                })
            });
    }

    fn example(path: &str) -> Url {
        Url::parse(&format!("https://example.com{path}")).unwrap()
    }

    fn known_products() -> KnownUrls {
        KnownUrls {
            products: vec![example("/product/9")],
            ..KnownUrls::default()
        }
    }

    fn frontier_repository(
        frontier: Vec<Url>,
        known: KnownUrls,
        replaced_with: Option<Vec<Url>>,
    ) -> Arc<MockCrawlFrontierRepository> {
        let mut repository = MockCrawlFrontierRepository::new();
        repository
            .expect_load_frontier()
            .times(1)
            .return_once(move |_| Box::pin(async move { Ok(frontier) }));
        repository
            .expect_known_urls()
            .return_once(move |_| Box::pin(async move { Ok(known) }));
        match replaced_with {
            Some(expected) => {
                repository
                    .expect_replace_frontier()
                    .withf(move |_, frontier| frontier == expected.as_slice())
                    .times(1)
                    .returning(|_, _| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
            }
            None => {
                repository.expect_replace_frontier().times(0);
            }
        }
        Arc::new(repository)
    }

    fn pattern_service_with_stored_pattern() -> MockUrlPatternService {
        let mut service = MockUrlPatternService::new();
        service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        service
    }

    #[tokio::test]
    async fn should_seed_an_incremental_crawl_and_keep_its_frontier_without_recording_a_full_crawl()
    {
        let listing_source_id = ListingSourceId::new();
        let mut mock_spider = MockSpider::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let mut pattern_service = pattern_service_with_stored_pattern();
        pattern_service.expect_mark_as_crawled().times(0);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);
        setup_mock_seeded_crawl(
            &mut mock_spider,
            CrawlRequest {
                seeds: CrawlSeeds {
                    frontier: vec![example("/deep/1")],
                    known: vec![example("/product/9")],
                    retired: vec![example("/collections/all/products/9")],
                },
                ..CrawlRequest::from_root("https://example.com", CrawlPace::default())
            },
            one_product_and_listing_pages(),
            CrawlDiscovery {
                frontier: vec![example("/deep/2")],
            },
        );
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(pattern_service),
            Arc::new(mock_url_repo),
            frontier_repository(
                vec![example("/deep/1")],
                KnownUrls {
                    products: vec![example("/product/9")],
                    retired: vec![example("/collections/all/products/9")],
                },
                Some(vec![example("/deep/2")]),
            ),
        );

        let result = service
            .run(
                &listing_source_id,
                "https://example.com",
                20,
                CrawlPace::default(),
                CrawlKind::Incremental,
            )
            .await
            .unwrap();

        assert_eq!(result.completion, CrawlCompletion::Completed);
        assert_eq!(result.product_urls_count, 1);
    }

    /// An incremental crawl of `paths` whose pages link `known_skipped` stored product pages it
    /// did not fetch again.
    fn setup_mock_incremental_crawl(
        mock: &mut MockSpider,
        paths: Vec<String>,
        known_skipped: usize,
    ) {
        mock.expect_crawl().times(1).return_once(move |_| {
            let (tx, rx) = mpsc::channel(paths.len().max(1));
            for path in paths {
                tx.try_send(CrawledPage::new(CrawledUrl::new(example(&path))))
                    .unwrap();
            }
            let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
            diagnostics_tx
                .send(CrawlDiagnostics {
                    known_skipped,
                    ..CrawlDiagnostics::default()
                })
                .unwrap();
            let (discovery_tx, discovery_rx) = oneshot::channel();
            discovery_tx.send(CrawlDiscovery::default()).unwrap();
            Box::pin(async {
                Ok(SpiderCrawl {
                    pages: rx,
                    diagnostics: diagnostics_rx,
                    discovery: discovery_rx,
                })
            })
        });
    }

    fn listing_pages() -> Vec<String> {
        (1..=20).map(|i| format!("/category/{i}")).collect()
    }

    #[tokio::test]
    async fn should_not_judge_an_incremental_crawl_on_the_stored_product_pages_it_skipped() {
        for paths in [listing_pages(), vec!["/".to_string()]] {
            let listing_source_id = ListingSourceId::new();
            let mut mock_spider = MockSpider::new();
            let mut mock_url_repo = MockUrlMetadataRepository::new();
            let mut pattern_service = pattern_service_with_stored_pattern();
            pattern_service.expect_classify_and_save().times(0);
            pattern_service.expect_mark_as_crawled().times(0);
            setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);
            setup_mock_incremental_crawl(&mut mock_spider, paths.clone(), 3);
            let service = SpiderServiceImpl::new(
                SpiderServiceConfig::default(),
                Box::new(mock_spider),
                Box::new(pattern_service),
                Arc::new(mock_url_repo),
                frontier_repository(Vec::new(), known_products(), Some(Vec::new())),
            );

            let result = service
                .run(
                    &listing_source_id,
                    "https://example.com",
                    20,
                    CrawlPace::default(),
                    CrawlKind::Incremental,
                )
                .await
                .unwrap_or_else(|error| panic!("{paths:?}: {error:?}"));

            assert_eq!(result.completion, CrawlCompletion::Completed);
            assert_eq!(result.product_urls_count, 0);
            assert_eq!(result.known_product_urls_skipped, 3);
            assert_eq!(result.product_pattern.as_deref(), Some("/product/"));
        }
    }

    #[tokio::test]
    async fn should_judge_an_incremental_crawl_that_met_no_stored_product_page_like_a_full_one() {
        let listing_source_id = ListingSourceId::new();
        let mut mock_spider = MockSpider::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let mut pattern_service = pattern_service_with_stored_pattern();
        pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        pattern_service.expect_mark_as_crawled().times(0);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);
        setup_mock_incremental_crawl(&mut mock_spider, listing_pages(), 0);
        // The run delivered pages, so its failure keeps the frontier it left: none.
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(pattern_service),
            Arc::new(mock_url_repo),
            frontier_repository(Vec::new(), known_products(), Some(Vec::new())),
        );

        let result = service
            .run(
                &listing_source_id,
                "https://example.com",
                20,
                CrawlPace::default(),
                CrawlKind::Incremental,
            )
            .await;

        assert!(
            matches!(result, Err(SpiderServiceError::NoProductUrls { .. })),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn should_record_a_full_crawl_that_left_no_frontier_and_clear_the_stored_one() {
        let listing_source_id = ListingSourceId::new();
        let mut mock_spider = MockSpider::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let mut pattern_service = pattern_service_with_stored_pattern();
        setup_mock_mark_as_crawled(&mut pattern_service, "https://example.com");
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);
        setup_mock_seeded_crawl(
            &mut mock_spider,
            CrawlRequest {
                seeds: CrawlSeeds {
                    frontier: vec![example("/deep/1")],
                    ..CrawlSeeds::default()
                },
                ..CrawlRequest::from_root("https://example.com", CrawlPace::default())
            },
            one_product_and_listing_pages(),
            CrawlDiscovery::default(),
        );
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(pattern_service),
            Arc::new(mock_url_repo),
            frontier_repository(
                vec![example("/deep/1")],
                KnownUrls::default(),
                Some(Vec::new()),
            ),
        );

        let result = service
            .run(
                &listing_source_id,
                "https://example.com",
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();

        assert_eq!(result.completion, CrawlCompletion::Completed);
    }

    #[tokio::test]
    async fn should_keep_the_previous_frontier_when_the_run_fails() {
        let mut mock_spider = MockSpider::new();
        let mut pattern_service = pattern_service_with_stored_pattern();
        pattern_service.expect_mark_as_crawled().times(0);
        setup_mock_seeded_crawl(
            &mut mock_spider,
            CrawlRequest {
                seeds: CrawlSeeds {
                    frontier: vec![example("/deep/1")],
                    ..CrawlSeeds::default()
                },
                ..CrawlRequest::from_root("https://example.com", CrawlPace::default())
            },
            vec!["/".to_string()],
            CrawlDiscovery {
                frontier: vec![example("/deep/1")],
            },
        );
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(pattern_service),
            Arc::new(MockUrlMetadataRepository::new()),
            frontier_repository(vec![example("/deep/1")], KnownUrls::default(), None),
        );

        let result = service
            .run(
                &ListingSourceId::new(),
                "https://example.com",
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::CrawlFailed { .. })
        ));
    }

    /// A full crawl of a ListingSource whose stored frontier is `/deep/1`, that delivers `paths`,
    /// ends with `diagnostics` and leaves `/deep/2` unvisited. `replaced` tells whether the run
    /// must store `/deep/2` as the ListingSource's frontier or leave `/deep/1` in place.
    async fn run_failing_crawl_with_frontier(
        paths: Vec<String>,
        diagnostics: CrawlDiagnostics,
        pattern_service: MockUrlPatternService,
        url_repository: MockUrlMetadataRepository,
        replaced: bool,
    ) -> SpiderServiceError {
        let mut spider = MockSpider::new();
        setup_mock_seeded_crawl_with_diagnostics(
            &mut spider,
            CrawlRequest {
                seeds: CrawlSeeds {
                    frontier: vec![example("/deep/1")],
                    ..CrawlSeeds::default()
                },
                ..CrawlRequest::from_root("https://example.com", CrawlPace::default())
            },
            paths,
            CrawlDiscovery {
                frontier: vec![example("/deep/2")],
            },
            diagnostics,
        );
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(spider),
            Box::new(pattern_service),
            Arc::new(url_repository),
            frontier_repository(
                vec![example("/deep/1")],
                KnownUrls::default(),
                replaced.then(|| vec![example("/deep/2")]),
            ),
        );

        service
            .run(
                &ListingSourceId::new(),
                "https://example.com",
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .expect_err("the run fails")
    }

    /// A pattern service for a ListingSource without a stored pattern whose classification of the
    /// crawl answers `classified`; the failed run records no full crawl.
    fn classifying_pattern_service(
        classified: Option<Result<Option<Regex>, UrlPatternServiceError>>,
    ) -> MockUrlPatternService {
        let mut service = MockUrlPatternService::new();
        service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        match classified {
            Some(classified) => {
                service
                    .expect_classify_and_save()
                    .times(1)
                    .return_once(move |_, _, _, _| Box::pin(async move { classified }));
            }
            None => {
                service.expect_classify_and_save().times(0);
            }
        }
        service.expect_mark_as_crawled().times(0);
        service
    }

    fn url_repository_storing(batches: usize) -> MockUrlMetadataRepository {
        let mut repository = MockUrlMetadataRepository::new();
        repository
            .expect_upsert_links_batch()
            .times(batches)
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });
        repository
    }

    /// A run that delivered pages but found no product URL among them keeps what it left
    /// unvisited, so the next crawl reaches further than this one.
    #[tokio::test]
    async fn should_replace_the_frontier_when_a_run_that_delivered_pages_finds_no_product_url() {
        let no_product_urls = run_failing_crawl_with_frontier(
            item_pages(),
            CrawlDiagnostics::default(),
            classifying_pattern_service(Some(Ok(Some(Regex::new(r"/product/").unwrap())))),
            url_repository_storing(1),
            true,
        )
        .await;
        assert!(
            matches!(no_product_urls, SpiderServiceError::NoProductUrls { .. }),
            "{no_product_urls:?}"
        );

        let no_url_pattern = run_failing_crawl_with_frontier(
            item_pages(),
            CrawlDiagnostics::default(),
            classifying_pattern_service(Some(Ok(None))),
            url_repository_storing(1),
            true,
        )
        .await;
        assert!(
            matches!(no_url_pattern, SpiderServiceError::NoUrlPattern { .. }),
            "{no_url_pattern:?}"
        );

        let small_sample = run_failing_crawl_with_frontier(
            vec!["/".to_string(), "/category/1".to_string()],
            CrawlDiagnostics::default(),
            classifying_pattern_service(None),
            url_repository_storing(0),
            true,
        )
        .await;
        assert!(
            matches!(
                small_sample,
                SpiderServiceError::InsufficientInferenceSample { sample_size: 2, .. }
            ),
            "{small_sample:?}"
        );
    }

    /// A rate-limit answer that stopped a crawl after it delivered pages does not change the
    /// frontier rule: the run keeps its frontier, the rate-limited page among it, and still
    /// carries the answer the scheduler holds the ListingSource for.
    #[tokio::test]
    async fn should_replace_the_frontier_and_carry_the_rate_limit_when_a_rate_limited_run_finds_no_product_url()
     {
        let rate_limit = RateLimitSignal {
            status: 429,
            retry_after: Some(std::time::Duration::from_secs(600)),
        };

        let error = run_failing_crawl_with_frontier(
            item_pages(),
            CrawlDiagnostics {
                completion: CrawlCompletion::RateLimited(rate_limit),
                rate_limit: Some(rate_limit),
                ..CrawlDiagnostics::default()
            },
            classifying_pattern_service(Some(Ok(None))),
            url_repository_storing(1),
            true,
        )
        .await;

        assert_eq!(error.rate_limit(), Some(rate_limit), "{error:?}");
        assert!(
            matches!(error.cause(), SpiderServiceError::NoUrlPattern { .. }),
            "{error:?}"
        );
    }

    /// A run whose pages were mostly bot challenges met the shop's bot protection: it fails as
    /// such instead of with the no-product-URL failure it would have recorded, which stays its
    /// source, and still keeps its frontier, the challenged pages among it.
    #[tokio::test]
    async fn should_fail_as_bot_protection_and_replace_the_frontier_when_most_pages_were_bot_challenges()
     {
        let mostly_challenges = || CrawlDiagnostics {
            pages_received: 20,
            bot_challenge_pages: 11,
            ..CrawlDiagnostics::default()
        };
        // A first classification of such pages neither opens a review nor runs inference: its
        // classifier is never asked, and the run stores none of its pages.
        let runs = [
            item_pages(),
            vec!["/".to_string(), "/category/1".to_string()],
        ];
        for paths in runs {
            let error = run_failing_crawl_with_frontier(
                paths,
                mostly_challenges(),
                classifying_pattern_service(None),
                url_repository_storing(0),
                true,
            )
            .await;

            let SpiderServiceError::BotChallengedCrawl {
                bot_challenge_pages: 11,
                pages_received: 20,
                failure,
            } = &error
            else {
                panic!("{error:?}");
            };
            let SpiderServiceError::InsufficientInferenceSample {
                stage: "end_of_crawl",
                diagnostics,
                ..
            } = &**failure
            else {
                panic!("{failure:?}");
            };
            assert_eq!(
                diagnostics.pages_received, 20,
                "the diagnostics stay attached"
            );
            assert_eq!(diagnostics.bot_challenge_pages, 11);
        }
    }

    /// The threshold comes before any diagnostics: a sample that is mostly challenges is not
    /// classified then, and the verdict is left to the end of the crawl.
    #[tokio::test]
    async fn should_not_classify_a_first_sample_at_the_threshold_when_most_pages_are_bot_challenges()
     {
        let mut mock_spider = MockSpider::new();
        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            "https://example.com",
            (0..6)
                .map(|i| format!("/challenge/{i}"))
                .chain((0..2).map(|i| format!("/item/{i}"))),
            received(8, 6),
        );
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(classifying_pattern_service(None)),
            Arc::new(url_repository_storing(0)),
            no_frontier(),
        );

        let error = service
            .run(
                &ListingSourceId::new(),
                "https://example.com",
                5,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .expect_err("the run fails");

        let SpiderServiceError::BotChallengedCrawl { failure, .. } = &error else {
            panic!("{error:?}");
        };
        // The skipped classification counts only the pages that were not challenges.
        assert!(
            matches!(
                &**failure,
                SpiderServiceError::InsufficientInferenceSample {
                    stage: "end_of_crawl",
                    sample_size: 2,
                    ..
                }
            ),
            "{failure:?}"
        );
    }

    /// The threshold judges the pages received so far: once challenges are no more than half of
    /// them, the crawl classifies its sample as it would have at the threshold itself.
    #[tokio::test]
    async fn should_classify_at_the_first_page_after_the_threshold_that_leaves_challenges_half_or_less()
     {
        let mut mock_spider = MockSpider::new();
        setup_mock_crawl(
            &mut mock_spider,
            "https://example.com",
            (0..3)
                .map(|i| format!("/challenge/{i}"))
                .chain((0..4).map(|i| format!("/item/{i}"))),
        );
        let mut pattern_service = MockUrlPatternService::new();
        pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        // Asked once, after the sixth page: three challenges of four, then of five, are most
        // of the pages, three of six are not.
        pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, _, sample| sample.len() == 6)
            .returning(|_, _, _, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
        setup_mock_mark_as_crawled(&mut pattern_service, "https://example.com");
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig {
                min_inference_sample_urls: 2,
                ..SpiderServiceConfig::default()
            },
            Box::new(mock_spider),
            Box::new(pattern_service),
            Arc::new(url_repository_storing(1)),
            no_frontier(),
        );

        let result = service
            .run(
                &ListingSourceId::new(),
                "https://example.com",
                4,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(result.is_ok(), "{result:?}");
    }

    /// A stored pattern that matched none of the pages of a run that were mostly bot challenges
    /// is not classified again: the challenges are no evidence against it, so the run opens no
    /// URL-pattern review, runs no inference, stores its pages under the stored pattern and fails
    /// as bot protection.
    #[tokio::test]
    async fn should_not_classify_a_stored_pattern_again_when_most_pages_were_bot_challenges() {
        let mut pattern_service = pattern_service_with_stored_pattern();
        pattern_service.expect_classify_and_save().times(0);
        pattern_service.expect_mark_as_crawled().times(0);

        let error = run_failing_crawl_with_frontier(
            item_pages(),
            received(20, 11),
            pattern_service,
            url_repository_storing(1),
            true,
        )
        .await;

        let SpiderServiceError::BotChallengedCrawl { failure, .. } = &error else {
            panic!("{error:?}");
        };
        assert!(
            matches!(**failure, SpiderServiceError::NoProductUrls { .. }),
            "{failure:?}"
        );
    }

    fn no_product_urls_failure() -> SpiderServiceError {
        SpiderServiceError::NoProductUrls {
            crawl_root_url: "https://example.com".to_string(),
            total_links: 20,
            product_pattern: "/product/".to_string(),
            completion: CrawlCompletion::Completed,
        }
    }

    fn received(pages_received: usize, bot_challenge_pages: usize) -> CrawlDiagnostics {
        CrawlDiagnostics {
            pages_received,
            bot_challenge_pages,
            ..CrawlDiagnostics::default()
        }
    }

    /// Bot challenges explain a run without product URLs only when they were more than half of
    /// its pages; below that share, and for any other failure, the run keeps its own.
    #[test]
    fn should_blame_bot_challenges_only_when_they_were_most_pages_of_a_run_without_product_urls() {
        let blamed = blame_bot_challenges(no_product_urls_failure(), &received(20, 11));
        assert!(
            matches!(blamed, SpiderServiceError::BotChallengedCrawl { .. }),
            "{blamed:?}"
        );

        for diagnostics in [received(20, 10), received(20, 0), received(0, 0)] {
            let kept = blame_bot_challenges(no_product_urls_failure(), &diagnostics);
            assert!(
                matches!(kept, SpiderServiceError::NoProductUrls { .. }),
                "{diagnostics:?}: {kept:?}"
            );
        }
        let challenged_root = blame_bot_challenges(
            SpiderServiceError::CrawlFailed {
                crawl_root_url: "https://example.com".to_string(),
                kind: CrawlFailureKind::BotProtection,
                total_links: 1,
                diagnostics: Box::new(received(1, 1)),
            },
            &received(1, 1),
        );
        assert!(
            matches!(
                challenged_root,
                SpiderServiceError::CrawlFailed {
                    kind: CrawlFailureKind::BotProtection,
                    ..
                }
            ),
            "{challenged_root:?}"
        );
    }

    /// A run that delivered no usable page (a challenged root, a rate limit before any page, an
    /// empty crawl) leaves the previous frontier in place, whatever its failure kind.
    #[tokio::test]
    async fn should_keep_the_previous_frontier_when_the_run_delivered_no_page() {
        for kind in [
            CrawlFailureKind::BotProtection,
            CrawlFailureKind::CloudflareChallenge,
            CrawlFailureKind::RateLimited,
            CrawlFailureKind::EmptyCrawl,
        ] {
            let error = run_failing_crawl_with_frontier(
                Vec::new(),
                CrawlDiagnostics {
                    failure_kind: Some(kind),
                    ..CrawlDiagnostics::default()
                },
                classifying_pattern_service(None),
                url_repository_storing(0),
                false,
            )
            .await;

            assert!(
                matches!(
                    error,
                    SpiderServiceError::CrawlFailed { kind: failed, total_links: 0, .. } if failed == kind
                ),
                "{error:?}"
            );
        }
    }

    /// A technical failure after the crawl delivered pages is no finding about the shop: the
    /// run keeps the previous frontier and the next one walks from it again.
    #[tokio::test]
    async fn should_keep_the_previous_frontier_when_classifying_the_delivered_pages_fails() {
        let error = run_failing_crawl_with_frontier(
            item_pages(),
            CrawlDiagnostics::default(),
            classifying_pattern_service(Some(Err(UrlPatternServiceError::Repository(
                sqlx::Error::PoolTimedOut,
            )))),
            url_repository_storing(0),
            false,
        )
        .await;

        assert!(
            matches!(error, SpiderServiceError::UrlPattern(_)),
            "{error:?}"
        );
    }

    /// A crawl that could not start, such as a root the transport cannot reach, never touches
    /// the frontier.
    #[tokio::test]
    async fn should_keep_the_previous_frontier_when_the_crawl_cannot_start() {
        let mut spider = MockSpider::new();
        spider.expect_crawl().times(1).return_once(|_| {
            Box::pin(async {
                Err(SpiderDiscoveryError::Preflight {
                    kind: CrawlFailureKind::ConnectError,
                    reason: "crawl root unreachable",
                    source: None,
                    root_reroot: None,
                })
            })
        });
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(spider),
            Box::new(classifying_pattern_service(None)),
            Arc::new(url_repository_storing(0)),
            frontier_repository(vec![example("/deep/1")], KnownUrls::default(), None),
        );

        let result = service
            .run(
                &ListingSourceId::new(),
                "https://example.com",
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(
            matches!(result, Err(SpiderServiceError::Discovery(_))),
            "{result:?}"
        );
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
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, links| links.len() == 25)
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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

    /// Re-reads of the ListingSource's pattern state find the review the crawl waits on undecided.
    fn setup_mock_review_still_pending(mock: &mut MockUrlPatternService) {
        mock.expect_decision_stored_during_crawl()
            .returning(|_, _| Box::pin(async { Ok(None) }));
    }

    #[tokio::test]
    async fn should_keep_urls_and_mark_crawled_when_pattern_review_is_pending_at_threshold() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let review_id = CrawlerReviewId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            product_pages_until_timeout(),
        );
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        // Asked once at the threshold; the pending review stops further asking.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        setup_mock_review_still_pending(&mut mock_pattern_service);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, links| {
                links.len() == 25
                    && links
                        .iter()
                        .all(|link| link.url_class != UrlClass::ProductListing)
            })
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        setup_mock_review_still_pending(&mut mock_pattern_service);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo
            .expect_upsert_links_batch()
            .times(1)
            .withf(|_, links| links.len() == 25)
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let review_id = CrawlerReviewId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, crawl_pattern, _| crawl_pattern.map(Regex::as_str) == Some(r"/product/"))
            .returning(move |_, _, _, _| {
                Box::pin(async move { Err(pending_review(listing_source_id, review_id)) })
            });
        setup_mock_review_still_pending(&mut mock_pattern_service);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();

        assert_eq!(run_result.pending_url_pattern_review, Some(review_id));
        assert_eq!(run_result.product_urls_count, 0);
        assert_eq!(run_result.product_pattern.as_deref(), Some(r"/product/"));
    }

    /// The pages a crawl that waits on a URL-pattern review stores, each batch with its classes.
    type StoredBatches = Arc<std::sync::Mutex<Vec<Vec<(String, UrlClass)>>>>;

    /// Runs a crawl of [`product_pages_until_timeout`] that opens a URL-pattern review at the
    /// threshold and stores batches of ten pages, whose re-reads of the ListingSource's pattern
    /// state find the review undecided until the last one, which finds `decision`.
    async fn run_crawl_decided_while_waiting(
        decision: DecidedUrlPattern,
    ) -> (Result<SpiderRunResult, SpiderServiceError>, StoredBatches) {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";
        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            product_pages_until_timeout(),
        );
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _| {
                Box::pin(
                    async move { Err(pending_review(listing_source_id, CrawlerReviewId::new())) },
                )
            });
        // Re-read after the threshold batch, then at the end of the crawl.
        let re_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        mock_pattern_service
            .expect_decision_stored_during_crawl()
            .times(2)
            .withf(|_, crawl_pattern| crawl_pattern.is_none())
            .returning(move |_, _| {
                let decided = re_reads.fetch_add(1, Ordering::SeqCst) == 1;
                let decision = decision.clone();
                Box::pin(async move { Ok(decided.then_some(decision)) })
            });
        mock_pattern_service
            .expect_mark_as_crawled()
            .returning(|_| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        let stored: StoredBatches = Arc::default();
        let recorded = Arc::clone(&stored);
        mock_url_repo
            .expect_upsert_links_batch()
            .returning(move |listing_source_id, links| {
                recorded.lock().unwrap().push(
                    links
                        .iter()
                        .map(|link| (link.url.path().to_owned(), link.url_class))
                        .collect(),
                );
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size: 10,
                ..SpiderServiceConfig::default()
            },
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        (run, stored)
    }

    /// A review approved while the crawl waits on it: the crawl stores every URL it stored while
    /// waiting again, classified with the approved pattern, and completes as a crawl with that
    /// pattern does.
    #[tokio::test]
    async fn should_classify_stored_urls_and_complete_when_review_is_approved_mid_crawl() {
        let (run, stored) = run_crawl_decided_while_waiting(DecidedUrlPattern::Pattern(
            Regex::new(r"/product/").unwrap(),
        ))
        .await;

        let run = run.unwrap_or_else(|error| panic!("crawl: {error}"));
        assert_eq!(run.pending_url_pattern_review, None);
        assert_eq!(run.completion, CrawlCompletion::Completed);
        assert_eq!(run.total_links, 25);
        assert_eq!(run.product_urls_count, 25);
        assert_eq!(run.product_pattern.as_deref(), Some(r"/product/"));
        let stored = stored.lock().unwrap();
        let (while_waiting, after_decision) = stored.split_at(2);
        assert!(
            while_waiting
                .iter()
                .flatten()
                .all(|(_, class)| *class != UrlClass::ProductListing),
            "{while_waiting:?}"
        );
        let classified: std::collections::BTreeSet<_> = after_decision
            .iter()
            .flatten()
            .filter(|(_, class)| *class == UrlClass::ProductListing)
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            classified.len(),
            25,
            "every URL the crawl stored is stored again as a product URL: {after_decision:?}"
        );
    }

    /// A review decided `NO_PATTERN` while the crawl waits on it ends the wait: the crawl records
    /// the failure of a ListingSource whose classification is complete without a pattern.
    #[tokio::test]
    async fn should_fail_with_no_url_pattern_when_review_decides_no_pattern_mid_crawl() {
        let (run, _) = run_crawl_decided_while_waiting(DecidedUrlPattern::NoPattern).await;

        assert!(
            matches!(
                run,
                Err(SpiderServiceError::NoUrlPattern {
                    total_links: 25,
                    completion: CrawlCompletion::Completed,
                    ..
                })
            ),
            "{run:?}"
        );
    }

    #[tokio::test]
    async fn should_fail_with_no_product_urls_when_pattern_matches_no_crawled_page() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);
        // The crawled URLs are kept, only the outcome is a failure.
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        // A decided `NO_PATTERN` classification answers with no pattern.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_mark_as_crawled().times(0);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();

        assert_eq!(run_result.completion, CrawlCompletion::PageBudgetReached);
    }

    fn rate_limited_completion() -> CrawlCompletion {
        CrawlCompletion::RateLimited(RateLimitSignal {
            status: 429,
            retry_after: None,
        })
    }

    #[test]
    fn should_log_rate_limited_run_as_rate_limited_not_as_hard_timeout() {
        let message = crawl_outcome_message(
            rate_limited_completion(),
            Some(CrawlStopReason::RateLimited),
            false,
        );

        assert!(message.contains("rate-limit"), "{message}");
        assert!(!message.contains("hard timeout"), "{message}");
        assert!(!message.contains("completed"), "{message}");
    }

    #[test]
    fn should_never_log_hard_timeout_run_as_completed() {
        for stopped_early in [None, Some(CrawlStopReason::RunEnded)] {
            let message =
                crawl_outcome_message(CrawlCompletion::HardTimeoutTruncated, stopped_early, false);

            assert!(message.contains("truncated"), "{message}");
            assert!(!message.contains("completed"), "{message}");
            assert_eq!(
                message.contains("stopped early"),
                stopped_early.is_some(),
                "{message}"
            );
        }
        for completion in [
            CrawlCompletion::Completed,
            CrawlCompletion::PageBudgetReached,
            CrawlCompletion::HardTimeoutTruncated,
            rate_limited_completion(),
        ] {
            for awaiting_review in [false, true] {
                let message = crawl_outcome_message(completion, None, awaiting_review);
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
    async fn should_return_only_after_the_library_crawl_stopped_when_the_run_fails_early() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mock_url_repo = MockUrlMetadataRepository::new();
        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";
        let library_stopped = Arc::new(AtomicBool::new(false));

        let stopped = Arc::clone(&library_stopped);
        mock_spider.expect_crawl().times(1).returning(move |_| {
            let (tx, rx) = mpsc::channel(25);
            let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
            let stopped = Arc::clone(&stopped);
            // Stands in for discovery: an endless crawl that stops once the
            // run drops the page channel and reports only after that.
            tokio::spawn(async move {
                for i in 1.. {
                    let url = Url::parse(&format!("https://example.com/item/{i}")).unwrap();
                    let page = CrawledPage::new(CrawledUrl::new(url));
                    if tx.send(page).await.is_err() {
                        break;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                stopped.store(true, Ordering::SeqCst);
                let _ = diagnostics_tx.send(CrawlDiagnostics {
                    completion: CrawlCompletion::HardTimeoutTruncated,
                    stopped_early: Some(CrawlStopReason::RunEnded),
                    ..CrawlDiagnostics::default()
                });
            });
            Box::pin(async move {
                Ok(SpiderCrawl {
                    pages: rx,
                    diagnostics: diagnostics_rx,
                    discovery: discovery_unknown(),
                })
            })
        });
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Err(UrlPatternServiceError::Repository(sqlx::Error::PoolClosed)) })
            });
        mock_pattern_service.expect_mark_as_crawled().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(
            matches!(result, Err(SpiderServiceError::UrlPattern(_))),
            "{result:?}"
        );
        assert!(
            library_stopped.load(Ordering::SeqCst),
            "the run returned while the library crawl was still running"
        );
    }

    #[tokio::test]
    async fn should_classify_at_end_if_threshold_not_reached() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));

        // It should classify at the end because threshold is above the crawl size.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, crawl_pattern, _| crawl_pattern.is_none())
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());

        // Persisted pattern expects /product/
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        // Reclassification gives the correct /item/ pattern
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .withf(|_, _, crawl_pattern, _| crawl_pattern.map(Regex::as_str) == Some(r"/product/"))
            .returning(|_, _, _, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                25,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let crawl_root_url = "https://example.com";

        // Fewer pages than one database batch, as a crawl paced by a
        // robots.txt Crawl-delay delivers.
        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());

        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        setup_mock_url_repo(&mut mock_url_repo, 1, listing_source_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                400,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();
        assert_eq!(run_result.product_urls_count, 20);
    }

    /// The spider stores a page a shop links with and without its trailing slash as one row, so
    /// it counts the page once, also when the two spellings arrive in different batches.
    #[tokio::test]
    async fn should_count_a_product_page_once_when_the_shop_links_both_trailing_slash_spellings() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";

        let mut paths = item_pages();
        paths.extend((1..=5).map(|i| format!("/item/{i}/")));
        setup_mock_crawl(&mut mock_spider, crawl_root_url, paths);

        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        mock_url_repo
            .expect_upsert_links_batch()
            .returning(|listing_source_id, links| {
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size: 4,
                ..SpiderServiceConfig::default()
            },
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                400,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();
        assert_eq!(run_result.total_links, 25);
        assert_eq!(run_result.product_urls_count, 20);
    }

    fn shop_page(path: &str, identity: PageIdentityEvidence) -> CrawledPage {
        CrawledPage::new(
            CrawledUrl::try_from(format!("https://example.com{path}").as_str()).unwrap(),
        )
        .with_identity(identity)
    }

    fn page_url(path: &str) -> CrawledUrl {
        CrawledUrl::try_from(format!("https://example.com{path}").as_str()).unwrap()
    }

    /// Every product page reaches storage with its canonical URL: from its canonical link, as a
    /// language copy, or by the Shopify rule the crawl applies once a page showed Shopify's
    /// markers, also to pages that carry no markers of their own, as long as the crawl may fetch
    /// the Shopify product URL.
    #[tokio::test]
    async fn should_store_each_duplicate_product_page_under_its_canonical_url() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";

        let mut pages = vec![
            shop_page(
                "/",
                PageIdentityEvidence {
                    platform: Some(ShopPlatform::Shopify),
                    ..PageIdentityEvidence::default()
                },
            ),
            shop_page(
                "/collections/all/products/ladle",
                PageIdentityEvidence {
                    canonical_link: Some(page_url("/products/ladle")),
                    ..PageIdentityEvidence::default()
                },
            ),
            shop_page(
                "/collections/sale/products/spoon",
                PageIdentityEvidence {
                    platform_product_urls: vec![(
                        ShopPlatform::Shopify,
                        page_url("/products/spoon"),
                    )],
                    ..PageIdentityEvidence::default()
                },
            ),
            shop_page(
                "/en/products/bowl",
                PageIdentityEvidence {
                    canonical_link: Some(page_url("/en/products/bowl")),
                    language_alternates: vec![
                        page_url("/products/bowl"),
                        page_url("/en/products/bowl"),
                    ],
                    platform: Some(ShopPlatform::Shopify),
                    ..PageIdentityEvidence::default()
                },
            ),
            shop_page("/products/ladle", PageIdentityEvidence::default()),
            shop_page("/collections/all", PageIdentityEvidence::default()),
        ];
        pages.extend(
            (1..20).map(|i| shop_page(&format!("/pages/{i}"), PageIdentityEvidence::default())),
        );
        mock_spider.expect_crawl().returning(move |_| {
            let pages = pages.clone();
            let (tx, rx) = mpsc::channel(50);
            tokio::spawn(async move {
                for page in pages {
                    tx.send(page).await.unwrap();
                }
            });
            let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
            diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
            Box::pin(async {
                Ok(SpiderCrawl {
                    pages: rx,
                    diagnostics: diagnostics_rx,
                    discovery: discovery_unknown(),
                })
            })
        });
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/products/[^/]+$").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        let stored: Arc<std::sync::Mutex<Vec<DiscoveredUrl>>> = Arc::default();
        let recorded = Arc::clone(&stored);
        mock_url_repo
            .expect_upsert_links_batch()
            .returning(move |listing_source_id, links| {
                recorded.lock().unwrap().extend_from_slice(links);
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let run_result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                400,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap();

        let canonical_of = |path: &str| {
            stored
                .lock()
                .unwrap()
                .iter()
                .find(|link| link.url.path() == path)
                .unwrap_or_else(|| panic!("{path} was not stored"))
                .canonical
                .as_ref()
                .map(|url| url.path().to_owned())
        };
        assert_eq!(
            canonical_of("/collections/all/products/ladle").as_deref(),
            Some("/products/ladle")
        );
        assert_eq!(
            canonical_of("/collections/sale/products/spoon").as_deref(),
            Some("/products/spoon")
        );
        assert_eq!(
            canonical_of("/en/products/bowl").as_deref(),
            Some("/products/bowl")
        );
        assert_eq!(canonical_of("/products/ladle"), None);
        assert_eq!(canonical_of("/collections/all"), None);
        assert_eq!(run_result.product_urls_count, 3, "each product counts once");
    }

    /// Crawls the 20 `/item/` pages under a stored `/item/` pattern while storage keeps the
    /// first `kept_other` of them `other`, as it keeps a page an operator rejected.
    /// A full crawl of `item_pages` whose storage keeps the first `kept_other` pages `other` and
    /// answers the next `retired` pages with rows retired as duplicates.
    async fn crawl_with_pages_kept_other(kept_other: usize, retired: usize) -> SpiderRunResult {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";
        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);
        let kept: Vec<String> = item_pages().into_iter().take(kept_other).collect();
        let duplicates: Vec<String> = item_pages()
            .into_iter()
            .skip(kept_other)
            .take(retired)
            .collect();
        mock_url_repo
            .expect_upsert_links_batch()
            .returning(move |listing_source_id, links| {
                let mut stored = stored_as_given(listing_source_id, links);
                for record in &mut stored {
                    if kept.iter().any(|path| path == record.url.path()) {
                        record.url_class = UrlClass::Other;
                    }
                    if duplicates.iter().any(|path| path == record.url.path()) {
                        record.disposition = CrawlerDisposition::Duplicate;
                    }
                }
                Box::pin(async move { Ok(stored) })
            });
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        service
            .run(
                &listing_source_id,
                crawl_root_url,
                400,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap()
    }

    /// The crawl reports the product URLs storage keeps, not every page the pattern matches.
    #[tokio::test]
    async fn should_count_only_the_product_urls_storage_keeps() {
        let run_result = crawl_with_pages_kept_other(3, 0).await;

        assert_eq!(run_result.total_links, 20);
        assert_eq!(run_result.product_urls_count, 17);
        assert_eq!(run_result.completion, CrawlCompletion::Completed);
    }

    /// A row retired as a duplicate keeps its product class, but its product is stored under its
    /// canonical URL, so the crawl does not count it.
    #[tokio::test]
    async fn should_not_count_rows_retired_as_duplicates_as_product_urls() {
        let run_result = crawl_with_pages_kept_other(0, 4).await;

        assert_eq!(run_result.product_urls_count, 16);
        assert_eq!(run_result.completion, CrawlCompletion::Completed);
    }

    /// Whether the pattern still fits the shop is decided by what it matches, so a crawl whose
    /// every match storage keeps `other` completes with the stored pattern instead of failing or
    /// inferring a new one.
    #[tokio::test]
    async fn should_keep_the_stored_pattern_when_storage_keeps_every_match_other() {
        let run_result = crawl_with_pages_kept_other(20, 0).await;

        assert_eq!(run_result.product_urls_count, 0);
        assert_eq!(run_result.completion, CrawlCompletion::Completed);
    }

    #[tokio::test]
    async fn should_return_empty_crawl_error_without_classifying_or_marking_crawled() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(first_full_crawl(crawl_root_url)))
            .returning(|_| {
                let (_tx, rx) = mpsc::channel(10);
                let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
                diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
                Box::pin(async {
                    Ok(SpiderCrawl {
                        pages: rx,
                        diagnostics: diagnostics_rx,
                        discovery: discovery_unknown(),
                    })
                })
            });

        mock_pattern_service
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/"]);

        mock_pattern_service
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            vec!["/collections", "/about"],
        );

        mock_pattern_service
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "end_of_crawl",
                sample_size: 2,
                min_sample_size: 20,
                ..
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_carry_the_rate_limit_answer_on_a_failure_after_a_rate_limited_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let crawl_root_url = "https://example.com";
        let rate_limit = RateLimitSignal {
            status: 429,
            retry_after: Some(std::time::Duration::from_secs(600)),
        };
        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            vec!["/"],
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::RateLimited),
                completion: CrawlCompletion::RateLimited(rate_limit),
                rate_limit: Some(rate_limit),
                ..CrawlDiagnostics::default()
            },
        );
        mock_pattern_service
            .expect_load_pattern()
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let error = service
            .run(
                &ListingSourceId::new(),
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap_err();

        assert_eq!(error.rate_limit(), Some(rate_limit), "{error:?}");
        assert!(
            matches!(
                error.cause(),
                SpiderServiceError::CrawlFailed {
                    kind: CrawlFailureKind::RateLimited,
                    total_links: 1,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn should_return_diagnostic_crawl_failure_for_rate_limited_tiny_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
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
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
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
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/item/1", "/item/2"]);

        mock_pattern_service
            .expect_load_pattern()
            .times(1)
            .returning(|_| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        let result = service
            .run(
                &listing_source_id,
                crawl_root_url,
                10,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "refresh",
                sample_size: 2,
                min_sample_size: 20,
                ..
            }) if url == crawl_root_url
        ));
    }

    /// The diagnostics of a crawl that a shop slowed with `Crawl-delay: 10` and whose links
    /// mostly left the crawl's host.
    fn slowed_crawl_leaving_its_host() -> CrawlDiagnostics {
        CrawlDiagnostics {
            completion: CrawlCompletion::HardTimeoutTruncated,
            pages_received: 9,
            accepted: 2,
            rejected_host: 7,
            status_counts: StatusCounts {
                success: 8,
                client_error: 1,
                ..StatusCounts::default()
            },
            robots_crawl_delay_ms: Some(10_000),
            request_delay_ms: 10_000,
            ..CrawlDiagnostics::default()
        }
    }

    /// The failure of a run on a ListingSource whose stored pattern is `stored_pattern`, whose
    /// crawl delivers two item pages and ends as `ended`.
    async fn run_failing_on_a_small_sample(
        stored_pattern: Option<&'static str>,
        classify_threshold: usize,
        ended: CrawlDiagnostics,
    ) -> SpiderServiceError {
        let crawl_root_url = "https://example.com";
        let mut spider = MockSpider::new();
        setup_mock_seeded_crawl_with_diagnostics(
            &mut spider,
            first_full_crawl(crawl_root_url),
            vec!["/item/1".to_string(), "/item/2".to_string()],
            CrawlDiscovery::default(),
            ended,
        );
        let mut pattern_service = MockUrlPatternService::new();
        pattern_service.expect_load_pattern().return_once(move |_| {
            Box::pin(async move { Ok(stored_pattern.map(|pattern| Regex::new(pattern).unwrap())) })
        });
        pattern_service.expect_classify_and_save().times(0);
        pattern_service.expect_mark_as_crawled().times(0);
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(spider),
            Box::new(pattern_service),
            Arc::new(url_repository_storing(0)),
            no_frontier(),
        );

        service
            .run(
                &ListingSourceId::new(),
                crawl_root_url,
                classify_threshold,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .expect_err("the run fails")
    }

    /// A sample too small to classify fails the run with what its crawl reported once it ended,
    /// whichever stage refused the sample: the threshold refuses it while the crawl still runs.
    /// A rate-limit answer that stopped the crawl wraps a failure that still carries them.
    #[tokio::test]
    async fn should_carry_the_ended_crawls_diagnostics_when_the_inference_sample_is_too_small() {
        let rate_limit = RateLimitSignal {
            status: 429,
            retry_after: Some(std::time::Duration::from_secs(600)),
        };
        let rate_limited = CrawlDiagnostics {
            completion: CrawlCompletion::RateLimited(rate_limit),
            rate_limit: Some(rate_limit),
            ..slowed_crawl_leaving_its_host()
        };
        for (stored_pattern, classify_threshold, expected, expected_stage) in [
            (None, 1, slowed_crawl_leaving_its_host(), "threshold"),
            (None, 25, slowed_crawl_leaving_its_host(), "end_of_crawl"),
            (
                Some(r"/product/"),
                25,
                slowed_crawl_leaving_its_host(),
                "refresh",
            ),
            (None, 25, rate_limited, "end_of_crawl"),
        ] {
            let failure =
                run_failing_on_a_small_sample(stored_pattern, classify_threshold, expected.clone())
                    .await;

            let SpiderServiceError::InsufficientInferenceSample {
                stage, diagnostics, ..
            } = failure.cause()
            else {
                panic!("{expected_stage}: expected a too small sample, got {failure:?}");
            };
            assert_eq!(*stage, expected_stage);
            assert_eq!(
                failure.rate_limit(),
                expected.rate_limit,
                "{expected_stage}"
            );
            assert_eq!(
                (
                    diagnostics.completion,
                    diagnostics.pages_received,
                    diagnostics.rejected_host,
                    diagnostics.status_counts,
                    diagnostics.robots_crawl_delay_ms,
                ),
                (
                    expected.completion,
                    expected.pages_received,
                    expected.rejected_host,
                    expected.status_counts,
                    expected.robots_crawl_delay_ms,
                ),
                "{expected_stage}"
            );
        }
    }

    /// A sample too small to classify among pages that were mostly bot challenges fails as bot
    /// protection, and the sample failure it replaces still carries what the crawl reported.
    #[tokio::test]
    async fn should_carry_the_ended_crawls_diagnostics_when_bot_challenges_take_the_blame() {
        let challenged = CrawlDiagnostics {
            bot_challenge_pages: 7,
            ..slowed_crawl_leaving_its_host()
        };

        let failure = run_failing_on_a_small_sample(None, 1, challenged).await;

        let SpiderServiceError::BotChallengedCrawl {
            bot_challenge_pages: 7,
            pages_received: 9,
            failure: replaced,
        } = &failure
        else {
            panic!("expected a bot-challenged crawl, got {failure:?}");
        };
        let SpiderServiceError::InsufficientInferenceSample { diagnostics, .. } = &**replaced
        else {
            panic!("expected a too small sample, got {replaced:?}");
        };
        assert_eq!(
            (
                diagnostics.pages_received,
                diagnostics.bot_challenge_pages,
                diagnostics.rejected_host,
                diagnostics.robots_crawl_delay_ms,
            ),
            (9, 7, 7, Some(10_000))
        );
    }

    /// Pages of a shop whose paths the keywords read as a category, an imprint and an info page,
    /// next to its item pages, enough to classify at the threshold.
    fn shop_pages() -> Vec<String> {
        let mut paths = item_pages();
        paths.extend(["/shop/chairs", "/impressum", "/about-us"].map(str::to_owned));
        paths
    }

    /// The classes a crawl of [`shop_pages`] stores with `url_classifier`, by path, while the
    /// ListingSource's pattern is `stored_pattern` or, without one, while its URL-pattern review is
    /// pending.
    async fn stored_classes(
        url_classifier: UrlClassifier,
        stored_pattern: Option<&'static str>,
    ) -> Vec<(String, UrlClass)> {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();
        let listing_source_id = ListingSourceId::new();
        let crawl_root_url = "https://example.com";
        setup_mock_crawl(&mut mock_spider, crawl_root_url, shop_pages());
        mock_pattern_service
            .expect_load_pattern()
            .returning(move |_| {
                Box::pin(
                    async move { Ok(stored_pattern.map(|pattern| Regex::new(pattern).unwrap())) },
                )
            });
        mock_pattern_service
            .expect_classify_and_save()
            .returning(move |_, _, _, _| {
                Box::pin(
                    async move { Err(pending_review(listing_source_id, CrawlerReviewId::new())) },
                )
            });
        setup_mock_review_still_pending(&mut mock_pattern_service);
        mock_pattern_service
            .expect_mark_as_crawled()
            .returning(|_| Box::pin(async { Ok(CrawlOutcomeWrite::Recorded) }));
        let stored = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = Arc::clone(&stored);
        mock_url_repo
            .expect_upsert_links_batch()
            .returning(move |listing_source_id, links| {
                recorded.lock().unwrap().extend(
                    links
                        .iter()
                        .map(|link| (link.url.path().to_owned(), link.url_class)),
                );
                let stored = stored_as_given(listing_source_id, links);
                Box::pin(async move { Ok(stored) })
            });
        let service = SpiderServiceImpl::new(
            SpiderServiceConfig {
                url_classifier,
                ..SpiderServiceConfig::default()
            },
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
            no_frontier(),
        );

        service
            .run(
                &listing_source_id,
                crawl_root_url,
                20,
                CrawlPace::default(),
                CrawlKind::Full,
            )
            .await
            .unwrap_or_else(|error| panic!("crawl: {error}"));

        stored.lock().unwrap().clone()
    }

    fn class_of(classes: &[(String, UrlClass)], path: &str) -> UrlClass {
        classes
            .iter()
            .find(|(stored, _)| stored == path)
            .map(|(_, class)| *class)
            .unwrap_or_else(|| panic!("{path} not stored: {classes:?}"))
    }

    /// In `agent` mode Claude decides what a product page is: until its pattern is approved the
    /// spider stores every crawled URL unclassified, where the other modes apply the keywords.
    #[tokio::test]
    async fn should_leave_crawled_urls_unclassified_in_agent_mode_until_a_pattern_is_approved() {
        let agent = stored_classes(UrlClassifier::PatternOnly, None).await;
        let other_modes = stored_classes(UrlClassifier::PatternAndKeywords, None).await;

        assert_eq!(agent.len(), shop_pages().len());
        assert!(
            agent.iter().all(|(_, class)| *class == UrlClass::Other),
            "{agent:?}"
        );
        assert_eq!(class_of(&other_modes, "/shop/chairs"), UrlClass::Category);
        assert_eq!(class_of(&other_modes, "/impressum"), UrlClass::Imprint);
        assert_eq!(class_of(&other_modes, "/about-us"), UrlClass::Info);
        assert_eq!(class_of(&other_modes, "/item/1"), UrlClass::Other);
    }

    /// Once Claude's pattern is approved, a crawl in `agent` mode classifies by it alone: what it
    /// takes is a product URL and the rest stays unclassified.
    #[tokio::test]
    async fn should_classify_by_the_approved_pattern_alone_in_agent_mode() {
        let agent = stored_classes(UrlClassifier::PatternOnly, Some("/item/")).await;
        let other_modes = stored_classes(UrlClassifier::PatternAndKeywords, Some("/item/")).await;

        for classes in [&agent, &other_modes] {
            assert_eq!(class_of(classes, "/item/7"), UrlClass::ProductListing);
        }
        for path in ["/shop/chairs", "/impressum", "/about-us"] {
            assert_eq!(class_of(&agent, path), UrlClass::Other, "{path}");
        }
        assert_eq!(class_of(&other_modes, "/shop/chairs"), UrlClass::Category);
    }
}

/// A crawl that delivered pages but no product URL is a failure, not a
/// completed crawl: the ListingSource's pattern matched nothing, or its
/// classification is complete without one.
fn no_product_urls_error(
    crawl_root_url: &str,
    state: &CrawlRunState,
    completion: CrawlCompletion,
) -> Option<SpiderServiceError> {
    if state.pattern_matched_any() {
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
/// diagnostic explains it when present; otherwise the size alone does. An
/// incremental crawl whose healthy root links product pages the ListingSource stores
/// reached the shop, although it fetched nothing else.
fn crawl_failure_error(
    crawl_root_url: &str,
    total_links: usize,
    skipped_known_products: usize,
    diagnostics: &CrawlDiagnostics,
) -> Option<SpiderServiceError> {
    let reached_known_products =
        total_links == 1 && skipped_known_products > 0 && diagnostics.failure_kind.is_none();
    if total_links > 1 || reached_known_products {
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
