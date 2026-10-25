use crate::authoring::CrawlerAuthoringMode;
use crate::review::model::ARTIFACT_PRODUCT_SCHEMA;
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::css_selector::removed_page_schema_repository::{
    NullRemovedPageSchemaRepository, RemovedPageSchemaRepository,
};
use crate::scraper::scraper_service::extraction::schema_review_gate::review_error_to_schema_service_error;
use crate::scraper::scraper_service::html_fetcher::HtmlFetcher;
use crate::scraper::scraper_service::image_validation::{ImageValidator, ReqwestImageValidator};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const DEFAULT_SCHEMA_SEED_PAGES: usize = 4;
pub const DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE: i64 = 20;

// ---------------------------------------------------------------------------
// ScraperServiceImpl struct
// ---------------------------------------------------------------------------

pub struct ScraperServiceImpl {
    pub(crate) html_fetcher: Box<dyn HtmlFetcher>,
    pub(crate) image_validator: Box<dyn ImageValidator>,
    pub(crate) schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
    pub(crate) candidate_service: Arc<dyn ScraperCandidateService>,
    pub(crate) removed_page_schema_repository: Box<dyn RemovedPageSchemaRepository + Send + Sync>,
    /// Number of HTML pages to seed first-time schema generation with.
    /// `1` means current page only; values >1 trigger best-effort sampling/fetch.
    pub(crate) schema_seed_pages: usize,
    /// Hard limit for total LLM calls per ListingSource across the whole scrape.
    pub(crate) max_llm_calls_per_listing_source: i64,
    /// Review gate for generated or operator-authored schemas; `None` persists generated
    /// schemas directly, which only tests use.
    pub(crate) review_repository: Option<CrawlerReviewRepository>,
    pub(crate) authoring: CrawlerAuthoringMode,
}

impl ScraperServiceImpl {
    pub fn new(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
    ) -> Self {
        Self::new_with_schema_seed_pages(
            html_fetcher,
            schema_service,
            candidate_service,
            DEFAULT_SCHEMA_SEED_PAGES,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        )
    }

    pub fn new_with_schema_seed_pages(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        schema_seed_pages: usize,
        max_llm_calls_per_listing_source: i64,
    ) -> Self {
        Self {
            html_fetcher,
            image_validator: Box::new(ReqwestImageValidator::new()),
            schema_service,
            candidate_service,
            removed_page_schema_repository: Box::new(NullRemovedPageSchemaRepository),
            schema_seed_pages: schema_seed_pages.max(1),
            max_llm_calls_per_listing_source,
            review_repository: None,
            authoring: CrawlerAuthoringMode::default(),
        }
    }

    /// Attaches the review gate that `authoring` needs: every mode records generated or
    /// operator-authored schemas as reviews before they become active.
    pub fn with_review_gate(
        mut self,
        review_repository: CrawlerReviewRepository,
        authoring: CrawlerAuthoringMode,
    ) -> Self {
        self.review_repository = Some(review_repository);
        self.authoring = authoring;
        self
    }

    /// The repository for manual schema reviews, when an operator authors schemas.
    pub(crate) fn manual_schema_review_repository(&self) -> Option<&CrawlerReviewRepository> {
        match self.authoring {
            CrawlerAuthoringMode::Manual => self.review_repository.as_ref(),
            CrawlerAuthoringMode::Auto | CrawlerAuthoringMode::Review => None,
        }
    }

    pub fn with_removed_page_schema_repository(
        mut self,
        repository: Box<dyn RemovedPageSchemaRepository + Send + Sync>,
    ) -> Self {
        self.removed_page_schema_repository = repository;
        self
    }

    pub(crate) async fn pending_product_schema_review_id(
        &self,
        listing_source_id: &listing_source_core::ListingSourceId,
    ) -> Result<Option<crate::CrawlerReviewId>, ProductListingSchemaServiceError> {
        let Some(review_repository) = &self.review_repository else {
            return Ok(None);
        };

        review_repository
            .latest_pending_review_id(listing_source_id, ARTIFACT_PRODUCT_SCHEMA)
            .await
            .map_err(ProductListingSchemaServiceError::DatabaseError)
    }

    /// Attaches a page that failed every cached schema to the ListingSource's pending
    /// product-schema review and returns that review, or `None` when none is pending and the
    /// page may open or generate one.
    pub(crate) async fn join_pending_product_schema_review(
        &self,
        listing_source_id: &listing_source_core::ListingSourceId,
        url: &url::Url,
        html: &str,
    ) -> Result<Option<crate::CrawlerReviewId>, ProductListingSchemaServiceError> {
        let Some(review_repository) = &self.review_repository else {
            return Ok(None);
        };

        review_repository
            .join_pending_schema_review(listing_source_id, url, html)
            .await
            .map_err(review_error_to_schema_service_error)
    }
}
