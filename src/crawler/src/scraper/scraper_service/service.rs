use crate::review::model::ARTIFACT_PRODUCT_SCHEMA;
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::css_selector::removed_page_schema_repository::{
    NullRemovedPageSchemaRepository, RemovedPageSchemaRepository,
};
use crate::scraper::normalization::product_normalization_service::ProductListingNormalizationService;
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
    pub(crate) normalization_service: Box<dyn ProductListingNormalizationService + Send + Sync>,
    pub(crate) candidate_service: Arc<dyn ScraperCandidateService>,
    pub(crate) removed_page_schema_repository: Box<dyn RemovedPageSchemaRepository + Send + Sync>,
    /// Number of HTML pages to seed first-time schema generation with.
    /// `1` means current page only; values >1 trigger best-effort sampling/fetch.
    pub(crate) schema_seed_pages: usize,
    /// Hard limit for total LLM calls per ListingSource across the whole scrape.
    pub(crate) max_llm_calls_per_listing_source: i64,
    pub(crate) review_repository: Option<CrawlerReviewRepository>,
    pub(crate) review_required: bool,
    pub(crate) schema_llm_review_mode: SchemaLlmReviewMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaLlmReviewMode {
    HumanOnly,
    ReportOnly,
    AutoApproveHighConfidence,
}

impl SchemaLlmReviewMode {
    pub fn from_env(review_required: bool) -> Self {
        let fallback = if review_required {
            Self::AutoApproveHighConfidence
        } else {
            Self::HumanOnly
        };
        std::env::var("CRAWLER_SCHEMA_LLM_REVIEW_MODE")
            .ok()
            .and_then(|raw| Self::parse(&raw))
            .unwrap_or(fallback)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "human_only" => Some(Self::HumanOnly),
            "report_only" => Some(Self::ReportOnly),
            "auto_approve_high_confidence" => Some(Self::AutoApproveHighConfidence),
            _ => None,
        }
    }

    pub(crate) fn allows_auto_approval(self) -> bool {
        matches!(self, Self::AutoApproveHighConfidence)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HumanOnly => "human_only",
            Self::ReportOnly => "report_only",
            Self::AutoApproveHighConfidence => "auto_approve_high_confidence",
        }
    }
}

impl ScraperServiceImpl {
    pub fn new(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductListingNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
    ) -> Self {
        Self::new_with_schema_seed_pages(
            html_fetcher,
            schema_service,
            normalization_service,
            candidate_service,
            DEFAULT_SCHEMA_SEED_PAGES,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        )
    }

    pub fn new_with_schema_seed_pages(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductListingNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        schema_seed_pages: usize,
        max_llm_calls_per_listing_source: i64,
    ) -> Self {
        Self {
            html_fetcher,
            image_validator: Box::new(ReqwestImageValidator::new()),
            schema_service,
            normalization_service,
            candidate_service,
            removed_page_schema_repository: Box::new(NullRemovedPageSchemaRepository),
            schema_seed_pages: schema_seed_pages.max(1),
            max_llm_calls_per_listing_source,
            review_repository: None,
            review_required: false,
            schema_llm_review_mode: SchemaLlmReviewMode::HumanOnly,
        }
    }

    pub fn with_review_gate(
        mut self,
        review_repository: CrawlerReviewRepository,
        review_required: bool,
    ) -> Self {
        self.review_repository = Some(review_repository);
        self.review_required = review_required;
        self.schema_llm_review_mode = SchemaLlmReviewMode::from_env(review_required);
        self
    }

    pub fn with_schema_llm_review_mode(mut self, mode: SchemaLlmReviewMode) -> Self {
        self.schema_llm_review_mode = mode;
        self
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
        if !self.review_required {
            return Ok(None);
        }

        let Some(review_repository) = &self.review_repository else {
            return Ok(None);
        };

        review_repository
            .latest_pending_review_id(listing_source_id, ARTIFACT_PRODUCT_SCHEMA)
            .await
            .map_err(ProductListingSchemaServiceError::DatabaseError)
    }
}

#[cfg(test)]
mod tests {
    use super::SchemaLlmReviewMode;

    #[test]
    fn parses_schema_llm_review_modes() {
        assert_eq!(
            SchemaLlmReviewMode::parse("human_only"),
            Some(SchemaLlmReviewMode::HumanOnly)
        );
        assert_eq!(
            SchemaLlmReviewMode::parse("report_only"),
            Some(SchemaLlmReviewMode::ReportOnly)
        );
        assert_eq!(
            SchemaLlmReviewMode::parse("auto_approve_high_confidence"),
            Some(SchemaLlmReviewMode::AutoApproveHighConfidence)
        );
        assert_eq!(SchemaLlmReviewMode::parse("unknown"), None);
    }

    #[test]
    fn auto_approval_is_only_allowed_for_high_confidence_mode() {
        assert!(!SchemaLlmReviewMode::ReportOnly.allows_auto_approval());
        assert!(SchemaLlmReviewMode::AutoApproveHighConfidence.allows_auto_approval());
    }
}
