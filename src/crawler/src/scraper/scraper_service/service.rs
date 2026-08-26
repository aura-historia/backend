use crate::network::policy::{
    NetworkAction, NetworkErrorKind, RetryPolicy, action_for, classify_reqwest_error,
    inline_retry_backoff_for,
};
use crate::review::model::ARTIFACT_PRODUCT_SCHEMA;
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaService;
use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
use crate::scraper::css_selector::removed_page_schema_repository::{
    NullRemovedPageSchemaRepository, RemovedPageSchemaRepository,
};
use crate::scraper::normalization::product_normalization_service::ProductListingNormalizationService;
use crate::scraper::scraper_service::auto_throttle::{
    ScraperAutoThrottle, ScraperAutoThrottleConfig,
};
use crate::scraper::scraper_service::image_validation::{ImageValidator, ReqwestImageValidator};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::debug;
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
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedHtml {
    pub html: String,
    pub final_url: Url,
}

impl FetchedHtml {
    pub fn new(html: String, final_url: Url) -> Self {
        Self { html, final_url }
    }
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
    auto_throttle: Arc<ScraperAutoThrottle>,
}

struct FetchAttemptError {
    error: FetchError,
}

impl Default for ReqwestHtmlFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHtmlFetcher {
    pub fn new() -> Self {
        Self::with_retry_policy_and_auto_throttle_config(
            RetryPolicy::default(),
            ScraperAutoThrottleConfig::default(),
        )
    }

    pub fn with_retry_policy(retry_policy: RetryPolicy) -> Self {
        Self::with_retry_policy_and_auto_throttle_config(
            retry_policy,
            ScraperAutoThrottleConfig::default(),
        )
    }

    pub fn with_auto_throttle_config(auto_throttle_config: ScraperAutoThrottleConfig) -> Self {
        Self::with_retry_policy_and_auto_throttle_config(
            RetryPolicy::default(),
            auto_throttle_config,
        )
    }

    pub fn with_retry_policy_and_auto_throttle_config(
        retry_policy: RetryPolicy,
        auto_throttle_config: ScraperAutoThrottleConfig,
    ) -> Self {
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
            .http1_only()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(DEFAULT_USER_AGENT)
            .default_headers(default_headers)
            .build()
            .expect("reqwest client should build");

        Self {
            client,
            retry_policy,
            auto_throttle: Arc::new(ScraperAutoThrottle::new(auto_throttle_config)),
        }
    }

    async fn wait_for_domain_slot(&self, domain: Option<&str>) {
        let Some(domain) = domain else {
            return;
        };

        let delay = self.auto_throttle.delay_for(domain);
        if delay.is_zero() {
            return;
        }

        debug!(
            domain,
            delay_ms = delay.as_millis(),
            "Applying scraper auto-throttle delay"
        );
        sleep(delay).await;
    }

    async fn fetch_once(&self, url: &Url) -> Result<FetchedHtml, FetchAttemptError> {
        let domain = url.host_str().map(str::to_owned);
        self.wait_for_domain_slot(domain.as_deref()).await;

        let started = Instant::now();
        let response = self.client.get(url.clone()).send().await.map_err(|err| {
            self.record_domain_latency(domain.as_deref(), started.elapsed());
            FetchAttemptError {
                error: FetchError::Network {
                    kind: classify_reqwest_error(&err),
                    details: err.to_string(),
                },
            }
        })?;

        self.record_domain_latency(domain.as_deref(), started.elapsed());

        let response = response
            .error_for_status()
            .map_err(|err| FetchAttemptError {
                error: FetchError::Network {
                    kind: classify_reqwest_error(&err),
                    details: err.to_string(),
                },
            })?;

        let final_url = response.url().clone();
        let html = response.text().await.map_err(|err| FetchAttemptError {
            error: FetchError::Network {
                kind: classify_reqwest_error(&err),
                details: err.to_string(),
            },
        })?;

        Ok(FetchedHtml::new(html, final_url))
    }

    fn record_domain_latency(&self, domain: Option<&str>, latency: Duration) {
        if let Some(domain) = domain {
            self.auto_throttle.record_latency(domain, latency);
            debug!(
                domain,
                latency_ms = latency.as_millis(),
                "Recorded scraper fetch latency"
            );
        }
    }
}

#[async_trait::async_trait]
impl HtmlFetcher for ReqwestHtmlFetcher {
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
        for attempt in 1..=self.retry_policy.max_attempts {
            match self.fetch_once(url).await {
                Ok(html) => return Ok(html),
                Err(attempt_error) => {
                    let action = action_for(attempt_error.error.kind());
                    if action != NetworkAction::Retry || attempt >= self.retry_policy.max_attempts {
                        return Err(attempt_error.error);
                    }
                    let backoff = inline_retry_backoff_for(self.retry_policy, attempt);
                    if !backoff.is_zero() {
                        sleep(backoff).await;
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
    pub(crate) schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
    pub(crate) normalization_service: Box<dyn ProductListingNormalizationService + Send + Sync>,
    pub(crate) candidate_service: Arc<dyn ScraperCandidateService>,
    pub(crate) removed_page_schema_repository: Box<dyn RemovedPageSchemaRepository + Send + Sync>,
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
            DEFAULT_MAX_LLM_CALLS_PER_SHOP,
        )
    }

    pub fn new_with_schema_seed_pages(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductListingSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductListingNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        schema_seed_pages: usize,
        max_llm_calls_per_shop: i64,
    ) -> Self {
        Self {
            html_fetcher,
            image_validator: Box::new(ReqwestImageValidator::new()),
            schema_service,
            normalization_service,
            candidate_service,
            removed_page_schema_repository: Box::new(NullRemovedPageSchemaRepository),
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

    pub fn with_removed_page_schema_repository(
        mut self,
        repository: Box<dyn RemovedPageSchemaRepository + Send + Sync>,
    ) -> Self {
        self.removed_page_schema_repository = repository;
        self
    }

    pub(crate) async fn pending_product_schema_review_id(
        &self,
        shop_id: &shop_core::shop_id::ShopId,
    ) -> Result<Option<uuid::Uuid>, ProductListingSchemaServiceError> {
        if !self.review_required {
            return Ok(None);
        }

        let Some(review_repository) = &self.review_repository else {
            return Ok(None);
        };

        review_repository
            .latest_pending_review_id(shop_id, ARTIFACT_PRODUCT_SCHEMA)
            .await
            .map_err(ProductListingSchemaServiceError::DatabaseError)
    }
}

#[cfg(test)]
mod tests {
    use super::{HtmlFetcher, ReqwestHtmlFetcher, SchemaLlmReviewMode};
    use crate::network::policy::RetryPolicy;
    use crate::scraper::scraper_service::ScraperAutoThrottleConfig;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use url::Url;

    async fn spawn_http_sequence(responses: Vec<&'static str>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            for response in responses {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        Url::parse(&format!("http://{addr}/product")).unwrap()
    }

    fn zero_delay_retry_policy(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    fn short_delay_retry_policy(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
        }
    }

    fn throttle_config(min_delay: Duration) -> ScraperAutoThrottleConfig {
        ScraperAutoThrottleConfig {
            target_concurrency: 2.0,
            min_delay,
            max_delay: min_delay,
            alpha: 0.15,
            enabled: true,
        }
    }

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

    #[tokio::test]
    async fn reqwest_fetcher_ignores_retry_after_for_inline_retry_delay() {
        let url = spawn_http_sequence(vec![
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 300\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;

        let fetcher = ReqwestHtmlFetcher::with_retry_policy_and_auto_throttle_config(
            zero_delay_retry_policy(2),
            throttle_config(Duration::ZERO),
        );

        let fetched = tokio::time::timeout(Duration::from_millis(100), fetcher.fetch(&url))
            .await
            .expect("retry should ignore long Retry-After for inline backoff")
            .unwrap();

        assert_eq!(fetched.html, "<html>ok</html>");
        assert_eq!(fetched.final_url, url);
    }

    #[tokio::test]
    async fn reqwest_fetcher_uses_short_inline_backoff_for_retryable_status() {
        let url = spawn_http_sequence(vec![
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;
        let fetcher = ReqwestHtmlFetcher::with_retry_policy_and_auto_throttle_config(
            short_delay_retry_policy(2),
            throttle_config(Duration::ZERO),
        );

        let fetched = tokio::time::timeout(Duration::from_millis(100), fetcher.fetch(&url))
            .await
            .expect("retry should use short inline backoff")
            .unwrap();

        assert_eq!(fetched.html, "<html>ok</html>");
        assert_eq!(fetched.final_url, url);
    }

    #[tokio::test]
    async fn reqwest_fetcher_applies_throttle_floor_before_fetch() {
        let url = spawn_http_sequence(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;
        let fetcher = ReqwestHtmlFetcher::with_retry_policy_and_auto_throttle_config(
            zero_delay_retry_policy(1),
            throttle_config(Duration::from_millis(25)),
        );

        let started = Instant::now();
        let fetched = fetcher.fetch(&url).await.unwrap();

        assert_eq!(fetched.html, "<html>ok</html>");
        assert_eq!(fetched.final_url, url);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }
}
