use crate::network::policy::{
    NetworkAction, NetworkErrorKind, RetryPolicy, action_for, backoff_delay,
    classify_reqwest_error, retry_cooldown_for,
};
use crate::review::model::ARTIFACT_PRODUCT_SCHEMA;
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::css_selector::product_schema_service::ProductSchemaService;
use crate::scraper::css_selector::product_schema_service::ProductSchemaServiceError;
use crate::scraper::normalization::product_normalization_service::ProductNormalizationService;
use crate::scraper::scraper_service::image_validation::{ImageValidator, ReqwestImageValidator};
use std::sync::Arc;
use tokio::time::sleep;
use url::Url;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const DEFAULT_SCHEMA_SEED_PAGES: usize = 4;
pub const DEFAULT_MAX_LLM_CALLS_PER_SHOP: i64 = 20;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";

// ---------------------------------------------------------------------------
// HtmlFetcher trait
// ---------------------------------------------------------------------------

/// Fetches raw HTML from a URL.  The real implementation delegates to
/// [`reqwest::Client`]; tests inject a fake.
#[async_trait::async_trait]
#[mockall::automock]
pub trait HtmlFetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<String, FetchError>;
}

// ---------------------------------------------------------------------------
// FetchError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("network failure: kind={kind:?}, details={details}")]
    Network {
        kind: NetworkErrorKind,
        details: String,
    },
}

impl FetchError {
    pub(crate) fn kind(&self) -> NetworkErrorKind {
        match self {
            FetchError::Network { kind, .. } => *kind,
        }
    }
}

// ---------------------------------------------------------------------------
// ReqwestHtmlFetcher
// ---------------------------------------------------------------------------

pub struct ReqwestHtmlFetcher {
    client: reqwest::Client,
    retry_policy: RetryPolicy,
}

impl Default for ReqwestHtmlFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHtmlFetcher {
    pub fn new() -> Self {
        Self::with_retry_policy(RetryPolicy::default())
    }

    pub fn with_retry_policy(retry_policy: RetryPolicy) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("de-DE,de;q=0.9,en-US;q=0.8,en;q=0.7"),
        );
        default_headers.insert(
            reqwest::header::CACHE_CONTROL,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );
        default_headers.insert(
            reqwest::header::PRAGMA,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(DEFAULT_USER_AGENT)
            .default_headers(default_headers)
            .build()
            .expect("reqwest client should build");

        Self {
            client,
            retry_policy,
        }
    }

    async fn fetch_once(&self, url: &Url) -> Result<String, FetchError> {
        let response =
            self.client
                .get(url.clone())
                .send()
                .await
                .map_err(|err| FetchError::Network {
                    kind: classify_reqwest_error(&err),
                    details: err.to_string(),
                })?;

        let response = response
            .error_for_status()
            .map_err(|err| FetchError::Network {
                kind: classify_reqwest_error(&err),
                details: err.to_string(),
            })?;

        response.text().await.map_err(|err| FetchError::Network {
            kind: classify_reqwest_error(&err),
            details: err.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl HtmlFetcher for ReqwestHtmlFetcher {
    async fn fetch(&self, url: &Url) -> Result<String, FetchError> {
        for attempt in 1..=self.retry_policy.max_attempts {
            match self.fetch_once(url).await {
                Ok(html) => return Ok(html),
                Err(err) => {
                    let action = action_for(err.kind());
                    if action != NetworkAction::Retry || attempt >= self.retry_policy.max_attempts {
                        return Err(err);
                    }
                    // For status codes with a dedicated cooldown (e.g. 429 → 1 sec),
                    // use that cooldown as the retry delay so the policy's max_delay
                    // cap doesn't truncate it to a much smaller value.
                    let cooldown = retry_cooldown_for(err.kind());
                    let backoff = backoff_delay(self.retry_policy, attempt);
                    let delay = cooldown.max(backoff);
                    if !delay.is_zero() {
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(FetchError::Network {
            kind: NetworkErrorKind::Unknown,
            details: "retry loop terminated unexpectedly".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// ScraperServiceImpl struct
// ---------------------------------------------------------------------------

pub struct ScraperServiceImpl {
    pub(crate) html_fetcher: Box<dyn HtmlFetcher>,
    pub(crate) image_validator: Box<dyn ImageValidator>,
    pub(crate) schema_service: Box<dyn ProductSchemaService + Send + Sync>,
    pub(crate) normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
    pub(crate) candidate_service: Arc<dyn ScraperCandidateService>,
    /// Maximum number of regenerate-and-retry attempts when no cached schema
    /// variant applies.
    pub(crate) max_schema_fix_attempts: u32,
    /// Number of HTML pages to seed first-time schema generation with.
    /// `1` means current page only; values >1 trigger best-effort sampling/fetch.
    pub(crate) schema_seed_pages: usize,
    /// Hard limit for total LLM calls per shop across the whole scrape.
    pub(crate) max_llm_calls_per_shop: i64,
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

    pub(crate) fn should_evaluate(self) -> bool {
        !matches!(self, Self::HumanOnly)
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
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        max_schema_fix_attempts: u32,
    ) -> Self {
        Self::new_with_schema_seed_pages(
            html_fetcher,
            schema_service,
            normalization_service,
            candidate_service,
            max_schema_fix_attempts,
            DEFAULT_SCHEMA_SEED_PAGES,
            DEFAULT_MAX_LLM_CALLS_PER_SHOP,
        )
    }

    pub fn new_with_schema_seed_pages(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        max_schema_fix_attempts: u32,
        schema_seed_pages: usize,
        max_llm_calls_per_shop: i64,
    ) -> Self {
        Self {
            html_fetcher,
            image_validator: Box::new(ReqwestImageValidator::new()),
            schema_service,
            normalization_service,
            candidate_service,
            max_schema_fix_attempts,
            schema_seed_pages: schema_seed_pages.max(1),
            max_llm_calls_per_shop,
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

    pub(crate) async fn pending_product_schema_review_id(
        &self,
        shop_id: &common::shop_id::ShopId,
    ) -> Result<Option<uuid::Uuid>, ProductSchemaServiceError> {
        if !self.review_required {
            return Ok(None);
        }

        let Some(review_repository) = &self.review_repository else {
            return Ok(None);
        };

        review_repository
            .latest_pending_review_id(shop_id, ARTIFACT_PRODUCT_SCHEMA)
            .await
            .map_err(ProductSchemaServiceError::DatabaseError)
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
        assert!(!SchemaLlmReviewMode::HumanOnly.should_evaluate());
        assert!(SchemaLlmReviewMode::ReportOnly.should_evaluate());
        assert!(!SchemaLlmReviewMode::ReportOnly.allows_auto_approval());
        assert!(SchemaLlmReviewMode::AutoApproveHighConfidence.should_evaluate());
        assert!(SchemaLlmReviewMode::AutoApproveHighConfidence.allows_auto_approval());
    }
}
