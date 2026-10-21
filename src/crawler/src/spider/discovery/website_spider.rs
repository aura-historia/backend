use application::error::BoxError;
use bloomfilter::Bloom;
use regex::Regex;
use reqwest::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};
use spider::CaseInsensitiveString;
use spider::packages::robotparser::parser::RobotFileParser;
use spider::page::AntiBotTech;
use spider::retry_strategy::{AttemptOutcome, RetryDirective, RetryStrategy};
use spider::tokio;
use spider::utils::auto_throttle::AutoThrottleConfig;
use spider::website::{CrawlStatus, Website, WebsiteMetaInfo};
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::network::CRAWLER_USER_AGENT;
use crate::network::policy::{
    NetworkAction, NetworkErrorKind, PublicTargetError, ReqwestErrorLike, RetryPolicy, action_for,
    classify_reqwest_error, inline_retry_backoff_for, is_same_or_www_host,
    network_error_kind_for_public_target_error, resolve_public_http_target,
};
use crate::spider::utils::url::CrawledUrl;

const SPIDER_ACCEPT_ENCODING: &str = "gzip, br, deflate";
const MAX_ROOT_REDIRECTS: usize = 5;
const SPIDER_MAX_BODY_BYTES_ENV: &str = "SPIDER_MAX_SIZE_BYTES";
const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_RETRY_COUNT: u8 = 2;
const DEFAULT_PREFLIGHT_ATTEMPTS: u32 = 2;
const DEFAULT_PREFLIGHT_DEADLINE: Duration = Duration::from_secs(90);

fn spider_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_static(SPIDER_ACCEPT_ENCODING),
    );
    headers
}

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
}

/// How long a domain should cool down after repeated failures of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlCooldownProfile {
    /// Flaky or temporary: the site may recover on its own.
    Transient,
    /// Needs a site or configuration change before a crawl can succeed.
    Recoverable,
    /// The site actively refuses this crawler.
    DurableBlock,
}

/// Stable crawl failure identifiers; `as_str()` values are persisted as
/// `listing_source_domains.last_crawl_error_kind` and must not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlFailureKind {
    EmptyCrawl,
    TinyCrawl,
    NoAcceptedPages,
    RobotsBlocked,
    RateLimited,
    AccessDenied,
    CloudflareChallenge,
    BotProtection,
    TlsError,
    ConnectError,
    ServerError,
    RedirectProblem,
    InvalidUrl,
    JavascriptRequired,
}

impl CrawlFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            CrawlFailureKind::EmptyCrawl => "EmptyCrawl",
            CrawlFailureKind::TinyCrawl => "TinyCrawl",
            CrawlFailureKind::NoAcceptedPages => "NoAcceptedPages",
            CrawlFailureKind::RobotsBlocked => "RobotsBlocked",
            CrawlFailureKind::RateLimited => "RateLimited",
            CrawlFailureKind::AccessDenied => "AccessDenied",
            CrawlFailureKind::CloudflareChallenge => "CloudflareChallenge",
            CrawlFailureKind::BotProtection => "BotProtection",
            CrawlFailureKind::TlsError => "TlsError",
            CrawlFailureKind::ConnectError => "ConnectError",
            CrawlFailureKind::ServerError => "ServerError",
            CrawlFailureKind::RedirectProblem => "RedirectProblem",
            CrawlFailureKind::InvalidUrl => "InvalidUrl",
            CrawlFailureKind::JavascriptRequired => "JavascriptRequired",
        }
    }

    pub const fn cooldown_profile(self) -> CrawlCooldownProfile {
        match self {
            CrawlFailureKind::EmptyCrawl
            | CrawlFailureKind::TinyCrawl
            | CrawlFailureKind::NoAcceptedPages
            | CrawlFailureKind::RateLimited
            | CrawlFailureKind::CloudflareChallenge
            | CrawlFailureKind::BotProtection
            | CrawlFailureKind::ConnectError
            | CrawlFailureKind::ServerError => CrawlCooldownProfile::Transient,
            CrawlFailureKind::RobotsBlocked
            | CrawlFailureKind::TlsError
            | CrawlFailureKind::RedirectProblem
            | CrawlFailureKind::InvalidUrl => CrawlCooldownProfile::Recoverable,
            CrawlFailureKind::AccessDenied | CrawlFailureKind::JavascriptRequired => {
                CrawlCooldownProfile::DurableBlock
            }
        }
    }
}

impl std::fmt::Display for CrawlFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a crawl that delivered pages ended. `as_str()` values are persisted as
/// `listing_source_domains.last_crawl_outcome` and must not change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CrawlCompletion {
    /// Spider ran out of links to follow.
    #[default]
    Completed,
    /// The configured page budget bounded discovery on purpose.
    PageBudgetReached,
    /// The whole-crawl wall-clock budget cut discovery short; the pages
    /// delivered before it are valid but the crawl is incomplete.
    HardTimeoutTruncated,
}

impl CrawlCompletion {
    pub const fn as_str(self) -> &'static str {
        match self {
            CrawlCompletion::Completed => "COMPLETED",
            CrawlCompletion::PageBudgetReached => "PAGE_BUDGET_REACHED",
            CrawlCompletion::HardTimeoutTruncated => "HARD_TIMEOUT_TRUNCATED",
        }
    }

    /// Whether discovery covered everything the configured bounds allow.
    pub const fn is_full(self) -> bool {
        !matches!(self, CrawlCompletion::HardTimeoutTruncated)
    }
}

impl std::fmt::Display for CrawlCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Spider reports a crawl cut short by its wall-clock budget as idle, like a
/// finished one, so elapsed time is the only signal. Spider starts that timer
/// after its own setup, so a crawl it stopped always took at least the budget
/// here. A reached page budget wins: discovery was bounded on purpose.
fn crawl_completion(
    elapsed: Duration,
    max_crawl_duration: Duration,
    pages_visited: usize,
    max_pages_per_crawl: u32,
) -> CrawlCompletion {
    if pages_visited >= max_pages_per_crawl as usize {
        CrawlCompletion::PageBudgetReached
    } else if elapsed >= max_crawl_duration {
        CrawlCompletion::HardTimeoutTruncated
    } else {
        CrawlCompletion::Completed
    }
}

/// Structured explanation of a crawl outcome. Counters are bounded by the
/// crawl budget; no URL collections are retained.
#[derive(Debug, Clone, Default)]
pub struct CrawlDiagnostics {
    /// How Spider stopped; only meaningful when `failure_kind` is unset.
    pub completion: CrawlCompletion,
    pub failure_kind: Option<CrawlFailureKind>,
    /// Status the established crawl root answered in preflight, before Spider
    /// ran. Spider never fetches the root when robots.txt refuses it.
    pub root_preflight_status: Option<u16>,
    /// Status of the root page after Spider's own retries.
    pub http_status: Option<u16>,
    pub final_url: Option<String>,
    pub redirect_url: Option<String>,
    pub diagnostic_reason: Option<String>,
    pub initial_html_length: Option<usize>,
    pub initial_should_retry: bool,
    pub initial_anti_bot: Option<String>,
    /// Pages Spider delivered to the local filter.
    pub pages_received: usize,
    /// Pages emitted downstream after local filtering.
    pub accepted: usize,
    pub rejected_host: usize,
    pub rejected_blacklist: usize,
    pub rejected_duplicate: usize,
    /// Links Spider refused because of robots.txt.
    pub robots_blocked: usize,
    pub first_robots_blocked_url: Option<String>,
    /// The robots.txt request itself was refused (401/403), which Spider
    /// treats as "disallow everything".
    pub robots_denied: bool,
    /// Pages dropped because the local filter fell behind Spider's broadcast.
    pub lagged: u64,
}

impl CrawlDiagnostics {
    fn apply_signal(&mut self, signal: DiagnosticSignal) {
        self.failure_kind = Some(signal.kind);
        self.diagnostic_reason = Some(signal.reason.to_string());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiagnosticSignal {
    kind: CrawlFailureKind,
    reason: &'static str,
}

impl DiagnosticSignal {
    const fn new(kind: CrawlFailureKind, reason: &'static str) -> Self {
        Self { kind, reason }
    }
}

#[derive(Debug)]
pub struct SpiderCrawl {
    pub pages: mpsc::Receiver<CrawledPage>,
    pub diagnostics: oneshot::Receiver<CrawlDiagnostics>,
}

#[derive(Debug, Error)]
pub enum SpiderDiscoveryError {
    /// Operator or build misconfiguration; not a property of the crawled domain.
    #[error("Spider configuration error: {0}")]
    Config(String),
    /// The HTTP client could not be built; not a property of the crawled domain.
    #[error("failed to build the spider HTTP client")]
    Client(#[source] BoxError),
    /// The crawl root could not be established before Spider started.
    #[error("Spider preflight failed ({kind}): {reason}")]
    Preflight {
        kind: CrawlFailureKind,
        reason: &'static str,
        #[source]
        source: Option<BoxError>,
    },
}

impl SpiderDiscoveryError {
    fn preflight(kind: CrawlFailureKind, reason: &'static str) -> Self {
        Self::Preflight {
            kind,
            reason,
            source: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum SpiderTransportError {
    #[error(transparent)]
    Target(#[from] PublicTargetError),
    #[error("failed to build the spider HTTP client")]
    Client(#[source] spider::reqwest::Error),
}

#[derive(Debug, Clone)]
pub struct CrawlerConfig {
    pub delay_millis: u64,
    pub request_timeout_secs: u64,
    pub concurrency_limit: usize,
    pub bloom_capacity: usize,
    pub bloom_fp_rate: f64,
    pub channel_size: usize,
    /// Maximum pages accepted in one crawl, including the root page.
    pub max_pages_per_crawl: u32,
    /// Whole-crawl wall-clock budget; request timeout remains per request.
    pub max_crawl_duration: Duration,
    /// Hard page-body ceiling enforced by Spider's streaming transport.
    pub max_response_body_bytes: usize,
    /// Spider retries per page for retryable responses and transport errors.
    pub retry_count: u8,
    /// Inline attempts for the root preflight on transient transport errors.
    pub preflight_retry: RetryPolicy,
    /// Wall-clock budget for the whole root preflight (redirects and retries).
    pub preflight_deadline: Duration,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            delay_millis: 500,
            request_timeout_secs: 15,
            concurrency_limit: 8,
            // Twice the page budget keeps false "duplicate" drops near `bloom_fp_rate`.
            bloom_capacity: 1_000_000,
            bloom_fp_rate: 0.001,
            channel_size: 1000,
            max_pages_per_crawl: 500_000,
            max_crawl_duration: Duration::from_secs(10 * 60),
            max_response_body_bytes: DEFAULT_MAX_RESPONSE_BODY_BYTES,
            retry_count: DEFAULT_RETRY_COUNT,
            preflight_retry: RetryPolicy {
                max_attempts: DEFAULT_PREFLIGHT_ATTEMPTS,
                ..RetryPolicy::default()
            },
            preflight_deadline: DEFAULT_PREFLIGHT_DEADLINE,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait Spider: Send + Sync {
    async fn crawl(&self, crawl_root_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError>;
}

/// Builds the HTTP client used for the crawl root and every Spider request.
///
/// The production transport resolves and pins public addresses; tests pin a
/// local listener so the preflight path runs unchanged.
#[async_trait::async_trait]
pub trait SpiderTransport: Send + Sync {
    async fn client_for(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> Result<spider::reqwest::Client, SpiderTransportError>;
}

pub struct PublicSpiderTransport;

#[async_trait::async_trait]
impl SpiderTransport for PublicSpiderTransport {
    async fn client_for(
        &self,
        url: &Url,
        timeout: Duration,
    ) -> Result<spider::reqwest::Client, SpiderTransportError> {
        let target = resolve_public_http_target(url, timeout).await?;
        spider_client_builder(timeout)
            .resolve_to_addrs(&target.host, &target.addresses)
            .build()
            .map_err(SpiderTransportError::Client)
    }
}

/// Spider applies its configured user agent and headers only to a client it
/// builds itself; an injected client must carry them as defaults, otherwise
/// every request (robots.txt included) goes out anonymous and shops that
/// reject anonymous clients answer 403.
fn spider_client_builder(timeout: Duration) -> spider::reqwest::ClientBuilder {
    spider::reqwest::Client::builder()
        .user_agent(CRAWLER_USER_AGENT)
        .default_headers(spider_request_headers())
        .redirect(spider::reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .no_proxy()
}

pub struct SpiderImpl {
    config: CrawlerConfig,
    transport: Arc<dyn SpiderTransport>,
}

impl SpiderImpl {
    pub fn new(config: CrawlerConfig) -> Self {
        Self::with_transport(config, Arc::new(PublicSpiderTransport))
    }

    pub fn with_transport(config: CrawlerConfig, transport: Arc<dyn SpiderTransport>) -> Self {
        Self { config, transport }
    }
}

impl Default for SpiderImpl {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

fn configured_spider_body_limit(
    configured: Option<&str>,
    maximum: usize,
) -> Result<(), SpiderDiscoveryError> {
    let configured = configured
        .ok_or_else(|| {
            SpiderDiscoveryError::Config(format!(
                "{SPIDER_MAX_BODY_BYTES_ENV} must be set to the crawler page-body limit"
            ))
        })?
        .parse::<usize>()
        .map_err(|_| {
            SpiderDiscoveryError::Config(format!(
                "{SPIDER_MAX_BODY_BYTES_ENV} must be an integer page-body limit"
            ))
        })?;
    if !(1_048_576..=maximum).contains(&configured) {
        return Err(SpiderDiscoveryError::Config(format!(
            "{SPIDER_MAX_BODY_BYTES_ENV} must be between 1048576 and {maximum} bytes"
        )));
    }
    Ok(())
}

fn configured_host_whitelist(url: &Url) -> Result<String, SpiderDiscoveryError> {
    let host = url.host_str().ok_or_else(|| {
        SpiderDiscoveryError::preflight(
            CrawlFailureKind::InvalidUrl,
            "configured crawler URL has no host",
        )
    })?;
    // Spider applies this filter after resolving extracted links to absolute
    // URLs. Keep it limited to the configured HTTP(S) host; the parsed URL
    // check below remains authoritative for the emitted crawl graph.
    Ok(format!(
        r"^https?://{}(?::(?:80|443))?(?:[/?#][^\\]*)?$",
        regex::escape(host)
    ))
}

fn root_redirect_target(
    configured_root: &Url,
    current_root: &Url,
    location: &str,
) -> Result<Url, SpiderDiscoveryError> {
    let redirect_target = current_root.join(location).map_err(|_| {
        SpiderDiscoveryError::preflight(
            CrawlFailureKind::RedirectProblem,
            "crawl-root redirect location is invalid",
        )
    })?;

    if !is_same_or_www_host(configured_root, &redirect_target) {
        return Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::RedirectProblem,
            "crawl-root redirect target is outside the configured bare/www host",
        ));
    }

    Ok(redirect_target)
}

/// Spider pins its own reqwest major version; route it through the shared
/// policy classifier.
impl ReqwestErrorLike for spider::reqwest::Error {
    fn is_timeout(&self) -> bool {
        spider::reqwest::Error::is_timeout(self)
    }

    fn is_connect(&self) -> bool {
        spider::reqwest::Error::is_connect(self)
    }

    fn is_request(&self) -> bool {
        spider::reqwest::Error::is_request(self)
    }

    fn status_code(&self) -> Option<u16> {
        self.status().map(|status| status.as_u16())
    }
}

/// reqwest reports TLS handshake failures as connect errors; the certificate
/// detail only survives in the messages of the underlying error chain. Only
/// the sources are inspected: the top-level message embeds the request URL,
/// which must not be able to trigger this classification.
fn is_tls_failure(error: &dyn std::error::Error) -> bool {
    std::iter::successors(error.source(), |source| source.source()).any(|source| {
        let detail = source.to_string().to_ascii_lowercase();
        ["certificate", "tls", "handshake"]
            .iter()
            .any(|needle| detail.contains(needle))
    })
}

struct PreflightAttemptError {
    error: SpiderDiscoveryError,
    retryable: bool,
}

fn preflight_transport_error(error: SpiderTransportError) -> PreflightAttemptError {
    match error {
        SpiderTransportError::Target(target_error) => {
            let network_kind = network_error_kind_for_public_target_error(target_error.clone());
            let kind = match network_kind {
                NetworkErrorKind::UnsafeTarget => CrawlFailureKind::InvalidUrl,
                _ => CrawlFailureKind::ConnectError,
            };
            PreflightAttemptError {
                retryable: action_for(network_kind) == NetworkAction::Retry,
                error: SpiderDiscoveryError::Preflight {
                    kind,
                    reason: "crawl-root target could not be resolved to a public address",
                    source: Some(Box::new(target_error)),
                },
            }
        }
        SpiderTransportError::Client(client_error) => PreflightAttemptError {
            retryable: false,
            error: SpiderDiscoveryError::Client(Box::new(client_error)),
        },
    }
}

fn preflight_request_error(error: spider::reqwest::Error) -> PreflightAttemptError {
    if is_tls_failure(&error) {
        return PreflightAttemptError {
            retryable: false,
            error: SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::TlsError,
                reason: "crawl-root TLS handshake failed",
                source: Some(Box::new(error)),
            },
        };
    }
    let network_kind = classify_reqwest_error(&error);
    PreflightAttemptError {
        retryable: action_for(network_kind) == NetworkAction::Retry,
        error: SpiderDiscoveryError::Preflight {
            kind: CrawlFailureKind::ConnectError,
            reason: "crawl-root request failed",
            source: Some(Box::new(error)),
        },
    }
}

/// A fetched crawl root together with the pinned client that fetched it, so
/// the crawl reuses the validated client instead of resolving the host again.
struct RootFetch {
    response: spider::reqwest::Response,
    client: spider::reqwest::Client,
}

async fn fetch_crawl_root_once(
    transport: &dyn SpiderTransport,
    root: &Url,
    timeout: Duration,
) -> Result<RootFetch, PreflightAttemptError> {
    let client = transport
        .client_for(root, timeout)
        .await
        .map_err(preflight_transport_error)?;
    let response = client
        .get(root.clone())
        .send()
        .await
        .map_err(preflight_request_error)?;
    Ok(RootFetch { response, client })
}

/// Fetches the crawl root with bounded inline retries for transient transport
/// failures only; HTTP statuses are never retried here because Spider owns
/// status-based retries for the root page.
async fn fetch_crawl_root(
    transport: &dyn SpiderTransport,
    root: &Url,
    timeout: Duration,
    retry: RetryPolicy,
) -> Result<RootFetch, SpiderDiscoveryError> {
    let max_attempts = retry.max_attempts.max(1);
    let mut attempt = 1;
    loop {
        match fetch_crawl_root_once(transport, root, timeout).await {
            Ok(fetched) => return Ok(fetched),
            Err(failure) if failure.retryable && attempt < max_attempts => {
                tokio::time::sleep(inline_retry_backoff_for(retry, attempt)).await;
                attempt += 1;
            }
            Err(failure) => return Err(failure.error),
        }
    }
}

/// The established crawl root, the status it answered and the client pinned
/// to it.
struct PreflightedRoot {
    url: Url,
    status: u16,
    client: spider::reqwest::Client,
}

/// Bounds the whole redirect chain, retries included, so a stalling host
/// cannot hold a spider slot for longer than `deadline`.
async fn preflight_crawl_root(
    transport: &dyn SpiderTransport,
    configured_root: Url,
    timeout: Duration,
    retry: RetryPolicy,
    deadline: Duration,
) -> Result<PreflightedRoot, SpiderDiscoveryError> {
    tokio::time::timeout(
        deadline,
        follow_crawl_root(transport, configured_root, timeout, retry),
    )
    .await
    .unwrap_or_else(|_| {
        Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::ConnectError,
            "crawl-root preflight exceeded its time budget",
        ))
    })
}

async fn follow_crawl_root(
    transport: &dyn SpiderTransport,
    configured_root: Url,
    timeout: Duration,
    retry: RetryPolicy,
) -> Result<PreflightedRoot, SpiderDiscoveryError> {
    let mut current_root = configured_root.clone();

    for redirect_count in 0..=MAX_ROOT_REDIRECTS {
        let RootFetch { response, client } =
            fetch_crawl_root(transport, &current_root, timeout, retry).await?;

        if !response.status().is_redirection() {
            return Ok(PreflightedRoot {
                url: current_root,
                status: response.status().as_u16(),
                client,
            });
        }

        if redirect_count == MAX_ROOT_REDIRECTS {
            break;
        }

        let location = response
            .headers()
            .get("location")
            .ok_or_else(|| {
                SpiderDiscoveryError::preflight(
                    CrawlFailureKind::RedirectProblem,
                    "crawl-root redirect response has no location",
                )
            })?
            .to_str()
            .map_err(|_| {
                SpiderDiscoveryError::preflight(
                    CrawlFailureKind::RedirectProblem,
                    "crawl-root redirect location is invalid",
                )
            })?;
        current_root = root_redirect_target(&configured_root, &current_root, location)?;
    }

    Err(SpiderDiscoveryError::preflight(
        CrawlFailureKind::RedirectProblem,
        "crawl-root redirect limit exceeded",
    ))
}

/// Bounded Spider retries that never re-fetch a body the transport ceiling
/// already truncated: the retry would truncate again.
struct TruncationAwareRetry {
    max_retries: u32,
}

impl RetryStrategy for TruncationAwareRetry {
    fn max_retries(&self) -> u32 {
        self.max_retries
    }

    fn on_retry(&self, outcome: &AttemptOutcome) -> RetryDirective {
        if outcome.content_truncated {
            RetryDirective::stop()
        } else {
            RetryDirective::default()
        }
    }
}

/// Library-level outcome handed from the crawl task to the filter task once
/// Spider has finished.
struct LibraryOutcome {
    completion: CrawlCompletion,
    status: CrawlStatus,
    meta: WebsiteMetaInfo,
    initial_html_length: usize,
    initial_should_retry: bool,
    initial_anti_bot: AntiBotTech,
    robots_denied: bool,
}

#[derive(Default)]
struct RobotsBlockedTracker {
    count: AtomicUsize,
    first_url: OnceLock<String>,
}

/// Decodes HTML character references in the links Spider finds, as a browser does for an
/// `href`. Spider reads attribute values raw, so `?c=&amp;id=113` would otherwise be requested
/// with an `amp;id` parameter and answered with the wrong page.
///
/// Spider admits a link (host whitelist, URL blacklist, robots.txt) on that raw text before it
/// calls the decoder, so the decoded URL is only handed back when it passes the same rules;
/// otherwise Spider fetches the raw link it admitted.
struct FoundLinkDecoder {
    host_whitelist: Regex,
    /// Set from Spider's own parser before the crawl starts; while unset, nothing is decoded.
    robots: OnceLock<Box<RobotFileParser>>,
}

impl FoundLinkDecoder {
    fn new(host_whitelist: &str) -> Result<Self, SpiderDiscoveryError> {
        let host_whitelist = Regex::new(host_whitelist).map_err(|_| {
            SpiderDiscoveryError::Config("Failed to compile host whitelist".to_string())
        })?;
        Ok(Self {
            host_whitelist,
            robots: OnceLock::new(),
        })
    }

    fn decode(&self, link: CaseInsensitiveString) -> CaseInsensitiveString {
        match html_escape::decode_html_entities(link.inner()) {
            Cow::Owned(decoded) if self.admits(&decoded) => decoded.into(),
            _ => link,
        }
    }

    fn admits(&self, url: &str) -> bool {
        self.host_whitelist.is_match(url)
            && !CrawledUrl::is_blacklisted_url(url)
            && self
                .robots
                .get()
                .is_some_and(|robots| robots.can_fetch(CRAWLER_USER_AGENT, url))
    }
}

impl SpiderImpl {
    async fn crawl_with_body_limit(
        &self,
        crawl_root_url: &str,
        configured_body_limit: Option<&str>,
    ) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        configured_spider_body_limit(configured_body_limit, self.config.max_response_body_bytes)?;
        let (tx, rx) = mpsc::channel(self.config.channel_size);
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let (diagnostics_tx, diagnostics_rx) = oneshot::channel();

        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderDiscoveryError::preflight(
                CrawlFailureKind::InvalidUrl,
                "configured crawler URL is invalid",
            )
        })?;
        let request_timeout = Duration::from_secs(self.config.request_timeout_secs);
        let PreflightedRoot {
            url: root_url,
            status: root_preflight_status,
            client,
        } = preflight_crawl_root(
            self.transport.as_ref(),
            configured_root,
            request_timeout,
            self.config.preflight_retry,
            self.config.preflight_deadline,
        )
        .await?;
        let host_whitelist = configured_host_whitelist(&root_url)?;
        let link_decoder = Arc::new(FoundLinkDecoder::new(&host_whitelist)?);
        let mut website = Website::new(root_url.as_str());
        website.set_http_client(client.clone());

        let blacklist_regex = CrawledUrl::blacklist_patterns();
        let auto_throttle_config = AutoThrottleConfig {
            min_delay_ms: self.config.delay_millis,
            max_delay_ms: 60_000,
            ..AutoThrottleConfig::default()
        };
        website
            .configuration
            .with_auto_throttle(auto_throttle_config);
        website
            .configuration
            .with_concurrency_limit(Some(self.config.concurrency_limit.max(1)));

        let robots_blocked = Arc::new(RobotsBlockedTracker::default());
        let robots_callback = Arc::clone(&robots_blocked);

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_whitelist_url(Some(vec![host_whitelist.into()]))
            .with_respect_robots_txt(true)
            .with_headers(Some(spider_request_headers()))
            .with_user_agent(Some(CRAWLER_USER_AGENT))
            .with_request_timeout(Some(request_timeout))
            .with_crawl_timeout(Some(self.config.max_crawl_duration))
            .with_limit(self.config.max_pages_per_crawl)
            .with_delay(self.config.delay_millis)
            .with_caching(false)
            .with_retry_strategy(Arc::new(TruncationAwareRetry {
                max_retries: u32::from(self.config.retry_count),
            }))
            .with_on_link_blocked_callback(Some(move |url: String| {
                robots_callback.count.fetch_add(1, Ordering::Relaxed);
                let _ = robots_callback.first_url.set(url);
            }))
            .with_on_link_find_callback(Some({
                let link_decoder = Arc::clone(&link_decoder);
                Arc::new(move |link, html| (link_decoder.decode(link), html))
            }))
            .build()
            .map_err(|_| SpiderDiscoveryError::Config("Failed to build website".to_string()))?;

        let mut spider_rx = website.subscribe(512);

        let max_crawl_duration = self.config.max_crawl_duration;
        let max_pages_per_crawl = self.config.max_pages_per_crawl;
        tokio::spawn(async move {
            let started = tokio::time::Instant::now();
            // Spider skips its own robots.txt fetch once the parser is loaded, so the
            // decoder checks decoded links against the rules Spider enforces.
            website.configure_robots_parser(&client).await;
            if let Some(robots) = website.get_robots_parser() {
                let _ = link_decoder.robots.set(robots.clone());
            }
            website.crawl().await;
            let outcome = LibraryOutcome {
                completion: crawl_completion(
                    started.elapsed(),
                    max_crawl_duration,
                    website.size(),
                    max_pages_per_crawl,
                ),
                status: *website.get_status(),
                meta: *website.get_website_meta_info(),
                initial_html_length: website.get_initial_html_length(),
                initial_should_retry: website.get_initial_page_should_retry(),
                initial_anti_bot: *website.get_initial_anti_bot_tech(),
                robots_denied: website
                    .get_robots_parser()
                    .as_ref()
                    .is_some_and(|robots| robots.disallow_all),
            };
            website.unsubscribe();
            let _ = outcome_tx.send(outcome);
        });

        let config = self.config.clone();
        let crawl_root_url = root_url.to_string();
        tokio::spawn(async move {
            let mut bloom = Bloom::new_for_fp_rate(config.bloom_capacity, config.bloom_fp_rate)
                .expect("bloom filter init failed");
            let mut diagnostics = CrawlDiagnostics {
                root_preflight_status: Some(root_preflight_status),
                ..CrawlDiagnostics::default()
            };
            let mut first_page_seen = false;
            let configured_root = Url::parse(&crawl_root_url).ok();
            let mut root_redirect_rejected = false;
            // Accepted URLs wait here instead of blocking the broadcast
            // subscription: Spider's 512-slot broadcast drops pages once the
            // subscriber falls behind, while this queue only holds small
            // normalized URLs and is bounded by the crawl page budget.
            let mut pending: VecDeque<CrawledPage> = VecDeque::new();
            let mut broadcast_open = true;

            while broadcast_open || !pending.is_empty() {
                // The consumer is gone; nothing left to deliver.
                if tx.is_closed() {
                    break;
                }
                let page = ::tokio::select! {
                    biased;
                    received = spider_rx.recv(), if broadcast_open => match received {
                        Ok(page) => page,
                        Err(RecvError::Lagged(skipped)) => {
                            diagnostics.lagged += skipped;
                            continue;
                        }
                        Err(RecvError::Closed) => {
                            broadcast_open = false;
                            continue;
                        }
                    },
                    permit = tx.reserve(), if !pending.is_empty() => match permit {
                        Ok(permit) => {
                            permit.send(pending.pop_front().expect("guarded by the select precondition"));
                            continue;
                        }
                        Err(_) => break,
                    },
                };
                diagnostics.pages_received += 1;

                if !first_page_seen {
                    apply_library_page(
                        &mut diagnostics,
                        &crawl_root_url,
                        page.get_url(),
                        page.status_code.as_u16(),
                        page.final_redirect_destination.as_deref(),
                        page.anti_bot_tech,
                    );
                    root_redirect_rejected = matches!(
                        diagnostics.failure_kind,
                        Some(CrawlFailureKind::RedirectProblem)
                    );
                    first_page_seen = true;
                }

                let raw_url = page.get_url();

                let Ok(parsed) = url::Url::parse(raw_url) else {
                    diagnostics.rejected_host += 1;
                    continue;
                };
                let normalized = CrawledUrl::new(parsed);

                if root_redirect_rejected
                    || !configured_root
                        .as_ref()
                        .is_some_and(|root| is_same_or_www_host(root, normalized.as_url()))
                {
                    diagnostics.rejected_host += 1;
                    continue;
                }

                if normalized.is_blacklisted() {
                    diagnostics.rejected_blacklist += 1;
                    continue;
                }

                let normalized_str = normalized.to_string();

                if bloom.check(&normalized_str) {
                    diagnostics.rejected_duplicate += 1;
                    continue;
                }
                bloom.set(&normalized_str);
                diagnostics.accepted += 1;
                pending.push_back(CrawledPage { url: normalized });
            }

            diagnostics.robots_blocked = robots_blocked.count.load(Ordering::Relaxed);
            diagnostics.first_robots_blocked_url = robots_blocked.first_url.get().cloned();
            match outcome_rx.await {
                Ok(outcome) => apply_library_outcome(&mut diagnostics, outcome),
                // The crawl task died without reporting; never call that full.
                Err(_) => {
                    diagnostics.completion = CrawlCompletion::HardTimeoutTruncated;
                    diagnostics.diagnostic_reason =
                        Some("crawl_task_ended_without_outcome".to_string());
                }
            }
            apply_local_filter_outcome(&mut diagnostics);
            let _ = diagnostics_tx.send(diagnostics);
        });

        Ok(SpiderCrawl {
            pages: rx,
            diagnostics: diagnostics_rx,
        })
    }
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(&self, crawl_root_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        let configured_body_limit = std::env::var(SPIDER_MAX_BODY_BYTES_ENV).ok();
        self.crawl_with_body_limit(crawl_root_url, configured_body_limit.as_deref())
            .await
    }
}

/// Records the root page Spider delivered: its status, where it ended up and
/// any failure signal the library attached to it.
fn apply_library_page(
    diagnostics: &mut CrawlDiagnostics,
    crawl_root_url: &str,
    page_url: &str,
    status_code: u16,
    final_redirect_destination: Option<&str>,
    anti_bot_tech: AntiBotTech,
) {
    let final_url = final_redirect_destination.unwrap_or(page_url).to_string();
    diagnostics.http_status = Some(status_code);
    diagnostics.redirect_url =
        (final_redirect_destination != Some(page_url)).then(|| final_url.clone());

    if let Some(signal) =
        page_diagnostic_signal(crawl_root_url, &final_url, status_code, anti_bot_tech)
    {
        diagnostics.apply_signal(signal);
    }
    diagnostics.final_url = Some(final_url);
}

fn page_diagnostic_signal(
    crawl_root_url: &str,
    final_url: &str,
    status_code: u16,
    anti_bot_tech: AntiBotTech,
) -> Option<DiagnosticSignal> {
    anti_bot_signal(status_code, anti_bot_tech)
        .or_else(|| status_code_signal(status_code))
        .or_else(|| redirect_signal(crawl_root_url, final_url))
}

/// A detected anti-bot vendor only explains a failure when the page was not
/// served: many healthy shops sit behind Cloudflare and answer 200.
fn anti_bot_signal(status_code: u16, anti_bot_tech: AntiBotTech) -> Option<DiagnosticSignal> {
    if (200..300).contains(&status_code) {
        return None;
    }
    match anti_bot_tech {
        AntiBotTech::Cloudflare => Some(DiagnosticSignal::new(
            CrawlFailureKind::CloudflareChallenge,
            "library_cloudflare_antibot",
        )),
        AntiBotTech::None => None,
        _ => Some(DiagnosticSignal::new(
            CrawlFailureKind::BotProtection,
            "library_bot_protection_antibot",
        )),
    }
}

fn status_code_signal(status_code: u16) -> Option<DiagnosticSignal> {
    match status_code {
        526 => Some(DiagnosticSignal::new(
            CrawlFailureKind::TlsError,
            "library_permanent_address_or_tls_error",
        )),
        525 => Some(DiagnosticSignal::new(
            CrawlFailureKind::ConnectError,
            "library_dns_or_connect_error",
        )),
        310 => Some(DiagnosticSignal::new(
            CrawlFailureKind::RedirectProblem,
            "library_too_many_redirects",
        )),
        _ => None,
    }
}

fn redirect_signal(crawl_root_url: &str, final_url: &str) -> Option<DiagnosticSignal> {
    let original = Url::parse(crawl_root_url).ok()?;
    let resolved = Url::parse(final_url).ok()?;

    (!is_same_or_www_host(&original, &resolved)).then_some(DiagnosticSignal::new(
        CrawlFailureKind::RedirectProblem,
        "library_redirect_to_unrelated_host",
    ))
}

fn apply_library_outcome(diagnostics: &mut CrawlDiagnostics, outcome: LibraryOutcome) {
    diagnostics.completion = outcome.completion;
    diagnostics.robots_denied = outcome.robots_denied;
    diagnostics.initial_html_length = Some(outcome.initial_html_length);
    diagnostics.initial_should_retry = outcome.initial_should_retry;
    diagnostics.initial_anti_bot = (outcome.initial_anti_bot != AntiBotTech::None)
        .then(|| format!("{:?}", outcome.initial_anti_bot));
    apply_website_status(diagnostics, outcome.status, outcome.meta);
}

fn apply_website_status(
    diagnostics: &mut CrawlDiagnostics,
    status: CrawlStatus,
    meta: WebsiteMetaInfo,
) {
    if diagnostics.failure_kind.is_some() {
        return;
    }

    if let Some(signal) = website_status_signal(status, meta) {
        diagnostics.apply_signal(signal);
    }
}

const CRAWL_ROOT_ACCESS_DENIED: &str = "crawl_root_access_denied";

const fn is_access_denied_status(status_code: u16) -> bool {
    matches!(status_code, 401 | 403)
}

/// Distinguishes "the site refused the crawler", "the site refused the
/// robots.txt request", "robots.txt blocked the crawl root" and "Spider
/// delivered pages but local filtering accepted none" from a genuinely empty
/// library response. Library signals keep precedence, so a 403 Spider
/// recognised as a JavaScript challenge or a vendor block stays that.
fn apply_local_filter_outcome(diagnostics: &mut CrawlDiagnostics) {
    if diagnostics.failure_kind.is_some() {
        return;
    }
    // The root Spider delivered, or the preflight answer when Spider never
    // fetched it (robots.txt refused or Spider delivered nothing).
    let root_status = diagnostics
        .http_status
        .or(diagnostics.root_preflight_status);
    if root_status.is_some_and(is_access_denied_status) {
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::AccessDenied,
            CRAWL_ROOT_ACCESS_DENIED,
        ));
    } else if diagnostics.pages_received == 0 && diagnostics.robots_denied {
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::AccessDenied,
            "robots_txt_request_denied",
        ));
    } else if diagnostics.pages_received == 0 && diagnostics.robots_blocked > 0 {
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::RobotsBlocked,
            "robots_blocked_crawl_root",
        ));
    } else if diagnostics.pages_received > 0 && diagnostics.accepted == 0 {
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::NoAcceptedPages,
            "local_filters_rejected_all_pages",
        ));
    }
}

fn website_status_signal(status: CrawlStatus, meta: WebsiteMetaInfo) -> Option<DiagnosticSignal> {
    match (status, meta) {
        (CrawlStatus::RateLimited, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::RateLimited,
            "library_rate_limited_status",
        )),
        (CrawlStatus::Blocked, WebsiteMetaInfo::RequiresJavascript) => Some(DiagnosticSignal::new(
            CrawlFailureKind::JavascriptRequired,
            "library_requires_javascript",
        )),
        (CrawlStatus::Blocked, _) | (CrawlStatus::FirewallBlocked, _) => Some(
            DiagnosticSignal::new(CrawlFailureKind::AccessDenied, "library_blocked_status"),
        ),
        (CrawlStatus::Empty, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::EmptyCrawl,
            "library_empty_status",
        )),
        (CrawlStatus::ConnectError, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::ConnectError,
            "library_connect_error_status",
        )),
        (CrawlStatus::ServerError, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::ServerError,
            "library_server_error_status",
        )),
        (CrawlStatus::Invalid, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::InvalidUrl,
            "library_invalid_url_status",
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn diagnostics_from_library_page(
        crawl_root_url: &str,
        page_url: &str,
        status_code: u16,
        final_redirect_destination: Option<&str>,
        anti_bot_tech: AntiBotTech,
    ) -> CrawlDiagnostics {
        let mut diagnostics = CrawlDiagnostics::default();
        apply_library_page(
            &mut diagnostics,
            crawl_root_url,
            page_url,
            status_code,
            final_redirect_destination,
            anti_bot_tech,
        );
        diagnostics
    }

    fn whitelist_for(root: &str) -> regex::Regex {
        let pattern = configured_host_whitelist(&Url::parse(root).unwrap()).unwrap();
        regex::Regex::new(&pattern).unwrap()
    }

    #[test]
    fn should_allow_only_configured_host_in_spider_fetch_graph() {
        let whitelist = whitelist_for("https://example.com");

        assert!(whitelist.is_match("https://example.com"));
        assert!(whitelist.is_match("https://example.com/"));
        assert!(whitelist.is_match("https://example.com/product/1"));
        assert!(whitelist.is_match("https://example.com/product/1?variant=x"));
        assert!(whitelist.is_match("https://example.com/product/1#reviews"));
        assert!(whitelist.is_match("https://example.com?sort=price"));

        assert!(!whitelist.is_match("https://evil.example/product/1"));
        assert!(!whitelist.is_match("https://example.com.evil.example/product/1"));
        assert!(!whitelist.is_match("https://www.example.com/product/1"));
        assert!(!whitelist.is_match("https://example.com/product\\1"));

        assert!(!whitelist.is_match("//evil.example/product/1"));
        assert!(!whitelist.is_match(r"/\evil.example/product/1"));
        assert!(!whitelist.is_match(r"\\evil.example/product/1"));

        assert!(!whitelist.is_match("mailto:test@example.com"));
        assert!(!whitelist.is_match("javascript:alert(1)"));
        assert!(!whitelist.is_match("data:text/html,test"));
        assert!(!whitelist.is_match("ftp://evil.example/file"));

        assert!(!whitelist.is_match("https://internal.example.com/product/1"));
        assert!(!whitelist.is_match("http://127.0.0.1/product/1"));
    }

    #[test]
    fn should_build_exact_www_host_graph_after_www_root_preflight() {
        let whitelist = whitelist_for("https://www.example.com/");

        assert!(whitelist.is_match("https://www.example.com/product/1"));
        assert!(!whitelist.is_match("https://example.com/product/1"));
    }

    /// Pins every test host to one local listener; the production preflight
    /// and crawl paths run unchanged on top of it.
    struct PinnedTransport {
        addr: SocketAddr,
        hosts: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl SpiderTransport for PinnedTransport {
        async fn client_for(
            &self,
            _url: &Url,
            timeout: Duration,
        ) -> Result<spider::reqwest::Client, SpiderTransportError> {
            let mut builder = spider_client_builder(timeout);
            for host in &self.hosts {
                builder = builder.resolve(host, self.addr);
            }
            builder.build().map_err(SpiderTransportError::Client)
        }
    }

    /// Transport that fails with a public-target policy error.
    struct RejectingTransport(PublicTargetError);

    #[async_trait::async_trait]
    impl SpiderTransport for RejectingTransport {
        async fn client_for(
            &self,
            _url: &Url,
            _timeout: Duration,
        ) -> Result<spider::reqwest::Client, SpiderTransportError> {
            Err(SpiderTransportError::Target(self.0.clone()))
        }
    }

    #[derive(Clone)]
    enum SiteResponse {
        Html(&'static str, String),
        Redirect(&'static str),
        /// Close the connection without answering, like a crashed server.
        Drop,
        /// Accept the request and never answer, like a stalled server.
        Hang,
    }

    /// `(host, target, user_agent, accept_encoding)` per request.
    type RequestLog = Arc<tokio::sync::Mutex<Vec<(String, String, String, String)>>>;
    type SiteHandler = Arc<dyn Fn(&str, usize) -> SiteResponse + Send + Sync>;

    struct TestSite {
        addr: SocketAddr,
        requests: RequestLog,
        server: tokio::task::JoinHandle<()>,
    }

    impl TestSite {
        async fn spawn(handler: SiteHandler) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = RequestLog::default();
            let server = tokio::spawn(serve_site(listener, requests.clone(), handler));
            Self {
                addr,
                requests,
                server,
            }
        }

        fn transport(&self, hosts: &[&'static str]) -> Arc<dyn SpiderTransport> {
            Arc::new(PinnedTransport {
                addr: self.addr,
                hosts: hosts.to_vec(),
            })
        }

        async fn requests_for(&self, target: &str) -> usize {
            self.requests
                .lock()
                .await
                .iter()
                .filter(|(_, seen, ..)| seen == target)
                .count()
        }

        async fn finish(self) -> Vec<(String, String, String, String)> {
            let log = self.requests.lock().await.clone();
            self.server.abort();
            log
        }
    }

    async fn serve_site(
        listener: tokio::net::TcpListener,
        requests: RequestLog,
        handler: SiteHandler,
    ) {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };

            let requests = requests.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut request = [0; 4096];
                let Ok(bytes_read) = stream.read(&mut request).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&request[..bytes_read]);
                let target = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let header = |name: &str| {
                    request
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case(name)
                                .then_some(value.trim().to_string())
                        })
                        .unwrap_or_default()
                };
                let host = header("host").to_ascii_lowercase();
                let user_agent = header("user-agent");
                let accept_encoding = header("accept-encoding");
                let previous = {
                    let mut log = requests.lock().await;
                    let previous = log.iter().filter(|(_, seen, ..)| *seen == target).count();
                    log.push((host, target.clone(), user_agent, accept_encoding));
                    previous
                };
                let response = match handler(&target, previous) {
                    SiteResponse::Html(status, body) => format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    SiteResponse::Redirect(location) => format!(
                        "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                    SiteResponse::Drop => return,
                    SiteResponse::Hang => {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                        return;
                    }
                };
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    }

    fn regression_site(target: &str, _previous: usize) -> SiteResponse {
        match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<!doctype html>
                    <a href="/product/1">Product</a>
                    <a href="category/1">Category</a>
                    <a href="?page=2">Next</a>
                    <a href="/category/">Nested category</a>
                    <a href="http://evil.example/product/1">External</a>"#
                    .to_string(),
            ),
            "/category/" | "/category" => SiteResponse::Html(
                "200 OK",
                r#"<a href="product/2">Nested product</a>"#.to_string(),
            ),
            "/product/1" | "/category/1" | "/?page=2" | "/category/product/2" => {
                SiteResponse::Html("200 OK", "<p>page</p>".to_string())
            }
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }
    }

    fn test_config() -> CrawlerConfig {
        CrawlerConfig {
            delay_millis: 0,
            request_timeout_secs: 2,
            concurrency_limit: 4,
            channel_size: 32,
            max_pages_per_crawl: 10,
            max_crawl_duration: Duration::from_secs(10),
            preflight_retry: RetryPolicy {
                max_attempts: 2,
                base_delay: Duration::from_millis(10),
                max_delay: Duration::from_millis(20),
            },
            ..CrawlerConfig::default()
        }
    }

    async fn run_crawl(
        spider: &SpiderImpl,
        root: &str,
    ) -> Result<(HashSet<String>, CrawlDiagnostics), SpiderDiscoveryError> {
        let SpiderCrawl {
            mut pages,
            diagnostics,
        } = spider.crawl_with_body_limit(root, Some("8388608")).await?;
        let mut discovered = HashSet::new();
        while let Some(page) = pages.recv().await {
            discovered.insert(page.url.to_string());
        }
        Ok((discovered, diagnostics.await.unwrap()))
    }

    #[tokio::test]
    async fn should_discover_relative_links_with_real_spider_impl() {
        let site = TestSite::spawn(Arc::new(regression_site)).await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "evil.example"]),
        );

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let request_log = site.finish().await;

        assert!(
            diagnostics.failure_kind.is_none(),
            "unexpected crawl diagnostics: {diagnostics:?}"
        );
        assert!(discovered.len() > 1, "only the root page was discovered");
        for expected in [
            "http://example.com/",
            "http://example.com/product/1",
            "http://example.com/category/1",
            "http://example.com/?page=2",
            "http://example.com/category/product/2",
        ] {
            assert!(discovered.contains(expected), "missing {expected}");
        }
        assert!(!discovered.contains("http://example.com/product/2"));
        assert!(!discovered.iter().any(|url| url.contains("evil.example")));
        assert_eq!(
            request_log
                .iter()
                .filter(|(host, ..)| host == "evil.example")
                .count(),
            0,
            "external host was requested: {request_log:?}"
        );
        // Preflight, robots.txt and every Spider page must identify the crawler;
        // anonymous clients are refused by common shop platforms.
        assert!(
            request_log
                .iter()
                .all(|(_, _, user_agent, _)| user_agent == CRAWLER_USER_AGENT),
            "request without the spider user agent: {request_log:?}"
        );
        // Spider's reqwest would otherwise advertise zstd, which fix(#1174)
        // excluded; the injected client must carry the same default.
        assert!(
            request_log
                .iter()
                .all(|(_, _, _, accept_encoding)| accept_encoding == SPIDER_ACCEPT_ENCODING),
            "request without the spider accept-encoding: {request_log:?}"
        );
        assert!(
            request_log
                .iter()
                .any(|(_, target, ..)| target == "/robots.txt")
        );
        assert_eq!(diagnostics.accepted, discovered.len());
        assert_eq!(diagnostics.pages_received, discovered.len());
        assert_eq!(diagnostics.http_status, Some(200));
        assert!(
            diagnostics
                .initial_html_length
                .is_some_and(|length| length > 0)
        );
        assert_eq!(diagnostics.rejected_host, 0);
        assert_eq!(diagnostics.lagged, 0);
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
    }

    fn entity_encoded_href_site(target: &str, _previous: usize) -> SiteResponse {
        match target {
            "/robots.txt" => {
                SiteResponse::Html("200 OK", "User-agent: *\nDisallow: /private/\n".to_string())
            }
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<!doctype html>
                    <a href="/single.asp?c=&amp;offset=20&amp;id=113">Product</a>
                    <a href="/shop?currency=EUR&amp;page=8">Currency page</a>
                    <a href="/index.php?category=26245&amp;page=3">Category page</a>
                    <a href="/literal?x=1&y=2">Literal ampersand</a>
                    <a href="/encoded?q=a%26b">Percent-encoded ampersand</a>
                    <a href="/private&sol;page">Robots-disallowed once decoded</a>
                    <a href="/post?p=1&amp;replytocom=55">Blacklisted once decoded</a>"#
                    .to_string(),
            ),
            _ => SiteResponse::Html("200 OK", "<p>page</p>".to_string()),
        }
    }

    #[tokio::test]
    async fn should_fetch_and_emit_entity_decoded_urls_when_hrefs_carry_html_entities() {
        let site = TestSite::spawn(Arc::new(entity_encoded_href_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, _) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let request_log = site.finish().await;

        let expected = [
            "http://example.com/single.asp?c=&id=113&offset=20",
            "http://example.com/shop?currency=EUR&page=8",
            "http://example.com/index.php?category=26245&page=3",
            "http://example.com/literal?x=1&y=2",
            "http://example.com/encoded?q=a%26b",
        ];
        for url in expected {
            assert!(discovered.contains(url), "missing {url} in {discovered:?}");
        }
        // The shop must receive the decoded query, or it renders the wrong page
        // and every link discovered from it inherits the mistake.
        for target in [
            "/single.asp?c=&offset=20&id=113",
            "/shop?currency=EUR&page=8",
            "/index.php?category=26245&page=3",
        ] {
            assert!(
                request_log
                    .iter()
                    .any(|(_, requested, ..)| requested == target),
                "missing request for {target}: {request_log:?}"
            );
        }
        // Decoding never turns a link Spider admitted into one that robots.txt or the
        // blacklist refuses.
        assert!(
            !request_log
                .iter()
                .any(|(_, target, ..)| target.starts_with("/private/")
                    || target.contains("&replytocom=")),
            "decoded URL bypassed robots.txt or the blacklist: {request_log:?}"
        );
    }

    fn link_decoder_with_robots(robots_txt: &str) -> FoundLinkDecoder {
        let root = Url::parse("https://www.example.com/").unwrap();
        let decoder = FoundLinkDecoder::new(&configured_host_whitelist(&root).unwrap()).unwrap();
        let mut robots = RobotFileParser::new();
        robots.parse(&robots_txt.lines().collect::<Vec<_>>());
        robots.modified();
        let _ = decoder.robots.set(robots);
        decoder
    }

    fn decode_link(decoder: &FoundLinkDecoder, link: &str) -> String {
        decoder.decode(link.into()).inner().to_string()
    }

    #[test]
    fn link_decoder_should_decode_character_references_seen_on_shops() {
        let decoder = link_decoder_with_robots("");

        for (link, expected) in [
            (
                "https://www.example.com/single.asp?c=&amp;offset=20&amp;id=113",
                "https://www.example.com/single.asp?c=&offset=20&id=113",
            ),
            (
                "https://www.example.com/shop?currency=EUR&amp;page=8",
                "https://www.example.com/shop?currency=EUR&page=8",
            ),
            (
                "https://www.example.com/index.php?category=26245&amp;page=3",
                "https://www.example.com/index.php?category=26245&page=3",
            ),
            (
                "https://www.example.com/literal?x=1&y=2",
                "https://www.example.com/literal?x=1&y=2",
            ),
            (
                "https://www.example.com/encoded?q=a%26b",
                "https://www.example.com/encoded?q=a%26b",
            ),
        ] {
            assert_eq!(decode_link(&decoder, link), expected);
        }
    }

    #[test]
    fn link_decoder_should_keep_raw_link_when_decoded_url_breaks_an_admission_rule() {
        let decoder = link_decoder_with_robots("User-agent: *\nDisallow: /private/\n");

        for link in [
            // robots.txt, which Spider matches on the path only
            "https://www.example.com/private&sol;page",
            // URL blacklist
            "https://www.example.com/post?p=1&amp;replytocom=55",
            // host whitelist, which refuses backslashes
            "https://www.example.com/a&bsol;..&bsol;admin",
        ] {
            assert_eq!(decode_link(&decoder, link), link);
        }
    }

    #[test]
    fn link_decoder_should_keep_raw_link_until_robots_txt_is_loaded() {
        let root = Url::parse("https://www.example.com/").unwrap();
        let decoder = FoundLinkDecoder::new(&configured_host_whitelist(&root).unwrap()).unwrap();
        let link = "https://www.example.com/shop?currency=EUR&amp;page=8";

        assert_eq!(decode_link(&decoder, link), link);
    }

    #[test]
    fn crawl_completion_should_distinguish_normal_budget_and_timeout_endings() {
        let budget = Duration::from_secs(600);

        assert_eq!(
            crawl_completion(Duration::from_secs(30), budget, 50, 10_000),
            CrawlCompletion::Completed
        );
        assert_eq!(
            crawl_completion(Duration::from_secs(30), budget, 10_000, 10_000),
            CrawlCompletion::PageBudgetReached
        );
        assert_eq!(
            crawl_completion(budget, budget, 4_000, 10_000),
            CrawlCompletion::HardTimeoutTruncated
        );
        assert_eq!(
            crawl_completion(budget + Duration::from_secs(1), budget, 10_000, 10_000),
            CrawlCompletion::PageBudgetReached
        );
    }

    #[test]
    fn crawl_completion_should_use_stable_persisted_identifiers() {
        assert_eq!(
            [
                CrawlCompletion::Completed,
                CrawlCompletion::PageBudgetReached,
                CrawlCompletion::HardTimeoutTruncated,
            ]
            .map(CrawlCompletion::as_str),
            ["COMPLETED", "PAGE_BUDGET_REACHED", "HARD_TIMEOUT_TRUNCATED"]
        );
        assert_eq!(
            serde_json::to_string(&CrawlCompletion::HardTimeoutTruncated).unwrap(),
            "\"HARD_TIMEOUT_TRUNCATED\""
        );
        assert!(CrawlCompletion::Completed.is_full());
        assert!(CrawlCompletion::PageBudgetReached.is_full());
        assert!(!CrawlCompletion::HardTimeoutTruncated.is_full());
    }

    #[tokio::test]
    async fn should_keep_pages_discovered_before_hard_timeout_and_report_truncation() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/product/1">1</a>
                   <a href="/product/2">2</a>
                   <a href="/slow">slow</a>"#
                    .to_string(),
            ),
            "/product/1" | "/product/2" => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
            "/slow" => SiteResponse::Hang,
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let config = CrawlerConfig {
            request_timeout_secs: 5,
            max_crawl_duration: Duration::from_millis(1_500),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        site.finish().await;

        assert_eq!(
            diagnostics.completion,
            CrawlCompletion::HardTimeoutTruncated
        );
        assert!(diagnostics.failure_kind.is_none(), "{diagnostics:?}");
        for expected in [
            "http://example.com/",
            "http://example.com/product/1",
            "http://example.com/product/2",
        ] {
            assert!(discovered.contains(expected), "missing {expected}");
        }
    }

    #[tokio::test]
    async fn should_report_page_budget_when_crawl_stops_at_configured_limit() {
        let site = TestSite::spawn(Arc::new(regression_site)).await;
        let config = CrawlerConfig {
            max_pages_per_crawl: 2,
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        site.finish().await;

        assert_eq!(diagnostics.completion, CrawlCompletion::PageBudgetReached);
        assert!(diagnostics.failure_kind.is_none(), "{diagnostics:?}");
        assert!(discovered.len() <= 2, "{discovered:?}");
    }

    #[tokio::test]
    async fn should_retry_preflight_once_after_dropped_connection() {
        let site = TestSite::spawn(Arc::new(|target: &str, previous: usize| {
            if target == "/" && previous == 0 {
                SiteResponse::Drop
            } else {
                regression_site(target, previous)
            }
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(discovered.contains("http://example.com/product/1"));
        assert!(diagnostics.failure_kind.is_none());
        // One dropped preflight, one successful preflight, one Spider root fetch.
        assert_eq!(site.requests_for("/").await, 3);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_fail_preflight_with_connect_error_after_retry_budget() {
        let site = TestSite::spawn(Arc::new(|_: &str, _: usize| SiteResponse::Drop)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::ConnectError,
                source: Some(_),
                ..
            }
        ));
        assert_eq!(site.requests_for("/").await, 2);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_bound_the_whole_preflight_by_its_deadline() {
        let site = TestSite::spawn(Arc::new(|_: &str, _: usize| SiteResponse::Hang)).await;
        let config = CrawlerConfig {
            preflight_deadline: Duration::from_millis(300),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));

        let started = tokio::time::Instant::now();
        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::ConnectError,
                reason: "crawl-root preflight exceeded its time budget",
                ..
            }
        ));
        // Well under one request timeout (2 s) of the stalled site.
        assert!(started.elapsed() < Duration::from_secs(2));
        site.finish().await;
    }

    #[tokio::test]
    async fn should_reuse_the_preflight_client_for_the_crawl() {
        let site = TestSite::spawn(Arc::new(regression_site)).await;
        let transport = Arc::new(CountingTransport {
            inner: PinnedTransport {
                addr: site.addr,
                hosts: vec!["example.com"],
            },
            builds: AtomicUsize::new(0),
        });
        let spider = SpiderImpl::with_transport(test_config(), transport.clone());

        let (discovered, _) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(discovered.contains("http://example.com/product/1"));
        // One client for the single preflight hop, none extra for the crawl.
        assert_eq!(transport.builds.load(Ordering::Relaxed), 1);
        site.finish().await;
    }

    struct CountingTransport {
        inner: PinnedTransport,
        builds: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl SpiderTransport for CountingTransport {
        async fn client_for(
            &self,
            url: &Url,
            timeout: Duration,
        ) -> Result<spider::reqwest::Client, SpiderTransportError> {
            self.builds.fetch_add(1, Ordering::Relaxed);
            self.inner.client_for(url, timeout).await
        }
    }

    #[tokio::test]
    async fn should_leave_status_retries_to_spider() {
        let site = TestSite::spawn(Arc::new(|target: &str, previous: usize| {
            if target == "/" && previous < 2 {
                SiteResponse::Html("503 Service Unavailable", String::new())
            } else {
                regression_site(target, previous)
            }
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(
            discovered.contains("http://example.com/product/1"),
            "spider should have retried the 503 root: {diagnostics:?}"
        );
        assert!(diagnostics.failure_kind.is_none());
        // Preflight accepts the 503 without retrying; Spider retries it once.
        assert_eq!(site.requests_for("/").await, 3);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_reject_off_host_root_redirect_without_retry() {
        let site = TestSite::spawn(Arc::new(|target: &str, previous: usize| {
            if target == "/" {
                SiteResponse::Redirect("https://other.example/")
            } else {
                regression_site(target, previous)
            }
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::RedirectProblem,
                ..
            }
        ));
        assert_eq!(site.requests_for("/").await, 1);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_bound_root_redirect_chain() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| {
            let hop: usize = target.trim_start_matches("/hop").parse().unwrap_or(0);
            SiteResponse::Redirect(Box::leak(format!("/hop{}", hop + 1).into_boxed_str()))
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::RedirectProblem,
                reason: "crawl-root redirect limit exceeded",
                ..
            }
        ));
        assert_eq!(site.requests.lock().await.len(), MAX_ROOT_REDIRECTS + 1);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_map_unsafe_target_to_invalid_url_without_retry() {
        let spider = SpiderImpl::with_transport(
            test_config(),
            Arc::new(RejectingTransport(PublicTargetError::UnsafeResolution)),
        );

        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::InvalidUrl,
                source: Some(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn should_map_dns_failure_to_connect_error() {
        let spider = SpiderImpl::with_transport(
            test_config(),
            Arc::new(RejectingTransport(PublicTargetError::ResolutionTimeout)),
        );

        let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::ConnectError,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn should_reject_invalid_root_url_before_any_request() {
        let site = TestSite::spawn(Arc::new(regression_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let error = run_crawl(&spider, "not a url").await.unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::InvalidUrl,
                ..
            }
        ));
        assert!(site.finish().await.is_empty());
    }

    #[tokio::test]
    async fn should_count_robots_blocked_links_without_emitting_them() {
        let site = TestSite::spawn(Arc::new(|target: &str, previous: usize| {
            if target == "/robots.txt" {
                SiteResponse::Html("200 OK", "User-agent: *\nDisallow: /product/\n".to_string())
            } else {
                regression_site(target, previous)
            }
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(!discovered.contains("http://example.com/product/1"));
        assert!(discovered.contains("http://example.com/category/1"));
        assert!(
            diagnostics.robots_blocked >= 1,
            "robots diagnostics missing: {diagnostics:?}"
        );
        assert!(
            diagnostics
                .first_robots_blocked_url
                .as_deref()
                .is_some_and(|url| url.contains("/product/1"))
        );
        assert_eq!(site.requests_for("/product/1").await, 0);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_report_robots_blocked_root_instead_of_empty_crawl() {
        for (robots, expected_kind) in [
            (
                SiteResponse::Html("200 OK", "User-agent: *\nDisallow: /\n".to_string()),
                CrawlFailureKind::RobotsBlocked,
            ),
            (
                SiteResponse::Html("403 Forbidden", String::new()),
                CrawlFailureKind::AccessDenied,
            ),
        ] {
            let site = TestSite::spawn(Arc::new(move |target: &str, previous: usize| {
                if target == "/robots.txt" {
                    robots.clone()
                } else {
                    regression_site(target, previous)
                }
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let (discovered, diagnostics) =
                run_crawl(&spider, "http://example.com/").await.unwrap();

            assert!(discovered.is_empty(), "{diagnostics:?}");
            assert_eq!(
                diagnostics.failure_kind,
                Some(expected_kind),
                "{diagnostics:?}"
            );
            assert_eq!(diagnostics.pages_received, 0);
            assert!(diagnostics.robots_blocked >= 1);
            assert_eq!(site.requests_for("/product/1").await, 0);
            site.finish().await;
        }
    }

    #[tokio::test]
    async fn should_report_site_refusing_the_crawler_as_root_access_denied() {
        let site = TestSite::spawn(Arc::new(|_: &str, _: usize| {
            SiteResponse::Html("403 Forbidden", String::new())
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let request_log = site.finish().await;

        assert!(discovered.is_empty(), "{diagnostics:?}");
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied),
            "{diagnostics:?}"
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some(CRAWL_ROOT_ACCESS_DENIED)
        );
        assert!(diagnostics.robots_denied);
        assert_eq!(diagnostics.root_preflight_status, Some(403));
        // Spider never fetched the root: robots.txt was refused first.
        assert_eq!(diagnostics.http_status, None);
        assert!(
            request_log
                .iter()
                .all(|(_, _, user_agent, _)| user_agent == CRAWLER_USER_AGENT),
            "{request_log:?}"
        );
    }

    #[tokio::test]
    async fn should_not_lose_pages_while_the_consumer_is_slow() {
        const LINKS: usize = 700;
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| {
            if target == "/" {
                let links: String = (0..LINKS)
                    .map(|i| format!(r#"<a href="/page/{i}">p</a>"#))
                    .collect();
                SiteResponse::Html("200 OK", links)
            } else {
                SiteResponse::Html("200 OK", "<p>page</p>".to_string())
            }
        }))
        .await;
        let config = CrawlerConfig {
            channel_size: 2,
            concurrency_limit: 16,
            max_pages_per_crawl: 2_000,
            max_crawl_duration: Duration::from_secs(60),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));
        let SpiderCrawl {
            mut pages,
            diagnostics,
        } = spider
            .crawl_with_body_limit("http://example.com/", Some("8388608"))
            .await
            .unwrap();

        // Let Spider finish the whole site (well past the 512-slot broadcast
        // capacity) while nobody drains the page channel.
        let served = |site: &TestSite| {
            let requests = site.requests.clone();
            async move { requests.lock().await.len() }
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while served(&site).await < LINKS + 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "site never finished"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let mut received = 0usize;
        while pages.recv().await.is_some() {
            received += 1;
        }
        let diagnostics = diagnostics.await.unwrap();
        site.finish().await;

        assert_eq!(diagnostics.lagged, 0, "broadcast lagged: {diagnostics:?}");
        assert_eq!(diagnostics.pages_received, LINKS + 1, "{diagnostics:?}");
        assert_eq!(received, LINKS + 1);
        assert_eq!(received, diagnostics.accepted);
        assert!(diagnostics.failure_kind.is_none());
    }

    #[test]
    fn should_resolve_relative_root_redirect_on_configured_host() {
        let configured = Url::parse("https://www.example.com/catalog").unwrap();
        let target = root_redirect_target(&configured, &configured, "/").unwrap();

        assert_eq!(target.as_str(), "https://www.example.com/");
    }

    #[test]
    fn should_allow_root_redirect_from_bare_host_to_www_host() {
        let configured = Url::parse("https://example.com/catalog").unwrap();
        let target =
            root_redirect_target(&configured, &configured, "https://www.example.com/").unwrap();

        assert_eq!(target.as_str(), "https://www.example.com/");
    }

    #[test]
    fn should_reject_root_redirect_to_unrelated_host() {
        let configured = Url::parse("https://example.com/catalog").unwrap();
        let error =
            root_redirect_target(&configured, &configured, "https://other.example/").unwrap_err();

        assert!(matches!(
            error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::RedirectProblem,
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .contains("outside the configured bare/www host")
        );
    }

    #[test]
    fn should_bound_root_redirects() {
        assert_eq!(MAX_ROOT_REDIRECTS, 5);
    }

    #[test]
    fn should_require_a_bounded_spider_transport_body_limit() {
        assert!(configured_spider_body_limit(Some("8388608"), 8 * 1024 * 1024).is_ok());
        assert!(matches!(
            configured_spider_body_limit(None, 8 * 1024 * 1024),
            Err(SpiderDiscoveryError::Config(_))
        ));
        assert!(configured_spider_body_limit(Some("not-a-number"), 8 * 1024 * 1024).is_err());
        assert!(configured_spider_body_limit(Some("8388609"), 8 * 1024 * 1024).is_err());
    }

    #[test]
    fn should_use_conservative_website_defaults() {
        let config = CrawlerConfig::default();

        assert_eq!(config.concurrency_limit, 8);
        assert_eq!(config.max_pages_per_crawl, 500_000);
        assert!(config.bloom_capacity >= 2 * config.max_pages_per_crawl as usize);
        assert_eq!(config.max_response_body_bytes, 8 * 1024 * 1024);
        assert_eq!(config.max_crawl_duration, Duration::from_secs(10 * 60));
        assert_eq!(config.retry_count, 2);
        assert_eq!(config.preflight_retry.max_attempts, 2);
        assert_eq!(config.preflight_deadline, Duration::from_secs(90));
    }

    #[test]
    fn should_stop_spider_retries_for_truncated_bodies_only() {
        let strategy = TruncationAwareRetry { max_retries: 2 };
        let outcome = |content_truncated: bool| AttemptOutcome {
            attempt: 1,
            status_code: spider::reqwest::StatusCode::BAD_GATEWAY,
            should_retry: true,
            content_truncated,
            waf_check: false,
            anti_bot_tech: &AntiBotTech::None,
            proxy_configured: false,
            url: "https://example.com/",
            profile_key: None,
            html_length: 0,
            bytes_transferred: None,
            error_status: None,
            final_redirect_destination: None,
        };

        assert_eq!(strategy.max_retries(), 2);
        assert!(!strategy.on_retry(&outcome(true)).should_retry);
        assert!(strategy.on_retry(&outcome(false)).should_retry);
    }

    #[tokio::test]
    async fn should_classify_spider_transport_errors_like_policy() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let client = spider_client_builder(Duration::from_secs(2))
            .resolve("tls-handshake.example", addr)
            .build()
            .unwrap();

        // The host name deliberately contains the TLS needles: the request URL
        // in the top-level message must never classify a connect failure as TLS.
        let refused = client
            .get("http://tls-handshake.example/certificate")
            .send()
            .await
            .unwrap_err();

        // Windows may surface a refused loopback connection as a timeout.
        assert!(matches!(
            classify_reqwest_error(&refused),
            NetworkErrorKind::Connect | NetworkErrorKind::Timeout
        ));
        assert!(!is_tls_failure(&refused));
        let failure = preflight_request_error(refused);
        assert!(failure.retryable);
        assert!(matches!(
            failure.error,
            SpiderDiscoveryError::Preflight {
                kind: CrawlFailureKind::ConnectError,
                source: Some(_),
                ..
            }
        ));
    }

    #[derive(Debug)]
    struct ChainedError {
        message: &'static str,
        source: Option<Box<ChainedError>>,
    }

    impl std::fmt::Display for ChainedError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for ChainedError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn std::error::Error + 'static))
        }
    }

    #[test]
    fn should_detect_tls_failures_only_in_the_error_source_chain() {
        let tls = ChainedError {
            message: "error sending request for url (https://shop.example/)",
            source: Some(Box::new(ChainedError {
                message: "client error (Connect)",
                source: Some(Box::new(ChainedError {
                    message: "invalid peer certificate: UnknownIssuer",
                    source: None,
                })),
            })),
        };
        assert!(is_tls_failure(&tls));

        let url_only = ChainedError {
            message: "error sending request for url (https://tls-handshake.example/)",
            source: Some(Box::new(ChainedError {
                message: "tcp connect error: connection refused",
                source: None,
            })),
        };
        assert!(!is_tls_failure(&url_only));
    }

    #[test]
    fn should_map_public_target_errors_to_preflight_kinds() {
        for (target_error, expected_kind, expected_retry) in [
            (
                PublicTargetError::ResolutionTimeout,
                CrawlFailureKind::ConnectError,
                true,
            ),
            (
                PublicTargetError::Resolution,
                CrawlFailureKind::ConnectError,
                true,
            ),
            (
                PublicTargetError::UnsafeResolution,
                CrawlFailureKind::InvalidUrl,
                false,
            ),
            (
                PublicTargetError::IpLiteral,
                CrawlFailureKind::InvalidUrl,
                false,
            ),
            (
                PublicTargetError::InvalidPort,
                CrawlFailureKind::InvalidUrl,
                false,
            ),
        ] {
            let failure = preflight_transport_error(SpiderTransportError::Target(target_error));
            assert_eq!(failure.retryable, expected_retry);
            assert!(matches!(
                failure.error,
                SpiderDiscoveryError::Preflight { kind, .. } if kind == expected_kind
            ));
        }
    }

    #[test]
    fn should_map_every_failure_kind_to_a_cooldown_profile() {
        use CrawlCooldownProfile::*;
        for (kind, profile) in [
            (CrawlFailureKind::EmptyCrawl, Transient),
            (CrawlFailureKind::TinyCrawl, Transient),
            (CrawlFailureKind::NoAcceptedPages, Transient),
            (CrawlFailureKind::RateLimited, Transient),
            (CrawlFailureKind::CloudflareChallenge, Transient),
            (CrawlFailureKind::BotProtection, Transient),
            (CrawlFailureKind::ConnectError, Transient),
            (CrawlFailureKind::ServerError, Transient),
            (CrawlFailureKind::RobotsBlocked, Recoverable),
            (CrawlFailureKind::TlsError, Recoverable),
            (CrawlFailureKind::RedirectProblem, Recoverable),
            (CrawlFailureKind::InvalidUrl, Recoverable),
            (CrawlFailureKind::AccessDenied, DurableBlock),
            (CrawlFailureKind::JavascriptRequired, DurableBlock),
        ] {
            assert_eq!(kind.cooldown_profile(), profile, "{kind}");
        }
    }

    fn page_diagnostics(url: &str, status_code: u16) -> CrawlDiagnostics {
        diagnostics_from_library_page(
            "https://example.com",
            url,
            status_code,
            None,
            AntiBotTech::None,
        )
    }

    fn library_outcome(status: CrawlStatus, meta: WebsiteMetaInfo) -> LibraryOutcome {
        LibraryOutcome {
            completion: CrawlCompletion::Completed,
            status,
            meta,
            initial_html_length: 0,
            initial_should_retry: false,
            initial_anti_bot: AntiBotTech::None,
            robots_denied: false,
        }
    }

    #[test]
    fn should_report_refused_robots_request_as_access_denied() {
        let mut diagnostics = CrawlDiagnostics {
            robots_blocked: 1,
            ..CrawlDiagnostics::default()
        };
        apply_library_outcome(
            &mut diagnostics,
            LibraryOutcome {
                robots_denied: true,
                ..library_outcome(CrawlStatus::Idle, WebsiteMetaInfo::None)
            },
        );
        apply_local_filter_outcome(&mut diagnostics);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied)
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some("robots_txt_request_denied")
        );
    }

    #[test]
    fn should_report_refused_robots_and_root_as_root_access_denied() {
        for root_status in [401, 403] {
            let mut diagnostics = CrawlDiagnostics {
                root_preflight_status: Some(root_status),
                robots_blocked: 1,
                ..CrawlDiagnostics::default()
            };
            apply_library_outcome(
                &mut diagnostics,
                LibraryOutcome {
                    robots_denied: true,
                    ..library_outcome(CrawlStatus::Idle, WebsiteMetaInfo::None)
                },
            );
            apply_local_filter_outcome(&mut diagnostics);

            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::AccessDenied)
            );
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some(CRAWL_ROOT_ACCESS_DENIED)
            );
            assert_eq!(diagnostics.root_preflight_status, Some(root_status));
            assert_eq!(diagnostics.http_status, None);
        }
    }

    #[test]
    fn should_report_refused_root_page_as_access_denied() {
        for status in [401, 403] {
            let mut diagnostics = CrawlDiagnostics {
                root_preflight_status: Some(status),
                pages_received: 1,
                accepted: 1,
                ..page_diagnostics("https://example.com", status)
            };
            apply_library_outcome(
                &mut diagnostics,
                library_outcome(CrawlStatus::Idle, WebsiteMetaInfo::None),
            );
            apply_local_filter_outcome(&mut diagnostics);

            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::AccessDenied)
            );
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some(CRAWL_ROOT_ACCESS_DENIED)
            );
            assert_eq!(diagnostics.http_status, Some(status));
        }
    }

    /// Spider's own reading of a 403 root explains more than the status
    /// alone; the generic access-denied signal must not hide it.
    #[test]
    fn should_keep_library_classification_of_a_refused_root() {
        for (status, meta, expected_kind, expected_reason) in [
            (
                CrawlStatus::Blocked,
                WebsiteMetaInfo::RequiresJavascript,
                CrawlFailureKind::JavascriptRequired,
                "library_requires_javascript",
            ),
            (
                CrawlStatus::Blocked,
                WebsiteMetaInfo::Apache403,
                CrawlFailureKind::AccessDenied,
                "library_blocked_status",
            ),
        ] {
            let mut diagnostics = CrawlDiagnostics {
                root_preflight_status: Some(403),
                pages_received: 1,
                accepted: 1,
                ..page_diagnostics("https://example.com", 403)
            };
            apply_library_outcome(&mut diagnostics, library_outcome(status, meta));
            apply_local_filter_outcome(&mut diagnostics);

            assert_eq!(diagnostics.failure_kind, Some(expected_kind));
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some(expected_reason)
            );
        }
    }

    #[test]
    fn should_report_refused_root_without_delivered_pages_as_access_denied() {
        let mut diagnostics = CrawlDiagnostics {
            root_preflight_status: Some(403),
            ..CrawlDiagnostics::default()
        };
        apply_library_outcome(
            &mut diagnostics,
            library_outcome(CrawlStatus::Idle, WebsiteMetaInfo::None),
        );
        apply_local_filter_outcome(&mut diagnostics);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied)
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some(CRAWL_ROOT_ACCESS_DENIED)
        );
    }

    #[test]
    fn should_flag_no_accepted_pages_only_without_library_signal() {
        let mut diagnostics = CrawlDiagnostics {
            pages_received: 3,
            accepted: 0,
            rejected_blacklist: 3,
            ..CrawlDiagnostics::default()
        };
        apply_local_filter_outcome(&mut diagnostics);
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::NoAcceptedPages)
        );

        let mut rate_limited = CrawlDiagnostics {
            pages_received: 1,
            accepted: 0,
            ..CrawlDiagnostics::default()
        };
        apply_library_outcome(
            &mut rate_limited,
            library_outcome(CrawlStatus::RateLimited, WebsiteMetaInfo::None),
        );
        apply_local_filter_outcome(&mut rate_limited);
        assert_eq!(
            rate_limited.failure_kind,
            Some(CrawlFailureKind::RateLimited)
        );

        let mut empty = CrawlDiagnostics::default();
        apply_local_filter_outcome(&mut empty);
        assert_eq!(empty.failure_kind, None);

        let mut accepted = CrawlDiagnostics {
            pages_received: 2,
            accepted: 1,
            ..CrawlDiagnostics::default()
        };
        apply_local_filter_outcome(&mut accepted);
        assert_eq!(accepted.failure_kind, None);
    }

    #[test]
    fn should_fill_initial_page_fields_from_library_outcome() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_library_outcome(
            &mut diagnostics,
            LibraryOutcome {
                completion: CrawlCompletion::HardTimeoutTruncated,
                status: CrawlStatus::Blocked,
                meta: WebsiteMetaInfo::None,
                initial_html_length: 1_234,
                initial_should_retry: true,
                initial_anti_bot: AntiBotTech::DataDome,
                robots_denied: false,
            },
        );

        // The status comes only from an actually delivered page.
        assert_eq!(diagnostics.http_status, None);
        assert_eq!(diagnostics.initial_html_length, Some(1_234));
        assert!(diagnostics.initial_should_retry);
        assert_eq!(
            diagnostics.completion,
            CrawlCompletion::HardTimeoutTruncated
        );
        assert_eq!(diagnostics.initial_anti_bot.as_deref(), Some("DataDome"));
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied)
        );
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/product/1").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/product/1");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/about").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/about");
    }

    #[test]
    fn should_not_request_zstd_when_building_spider_headers() {
        let headers = spider_request_headers();
        let accept_encoding = headers
            .get(reqwest::header::ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok());

        assert_eq!(accept_encoding, Some("gzip, br, deflate"));
    }

    #[test]
    fn should_accept_redirect_from_bare_host_to_www_host() {
        let original = Url::parse("https://moeblinger.de").unwrap();
        let resolved = Url::parse("https://www.moeblinger.de/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_accept_redirect_from_www_host_to_bare_host() {
        let original = Url::parse("https://www.example.com").unwrap();
        let resolved = Url::parse("https://example.com/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_subdomain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://catalog.example.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_domain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://other.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_map_rate_limited_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::RateLimited,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RateLimited)
        );
    }

    #[test]
    fn should_map_blocked_library_status_to_access_denied() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Blocked,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied)
        );
    }

    #[test]
    fn should_map_javascript_required_library_metadata() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Blocked,
            WebsiteMetaInfo::RequiresJavascript,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::JavascriptRequired)
        );
    }

    #[test]
    fn should_map_empty_library_status_to_empty_crawl() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(&mut diagnostics, CrawlStatus::Empty, WebsiteMetaInfo::None);

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::EmptyCrawl));
    }

    #[test]
    fn should_map_connect_error_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::ConnectError,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ConnectError)
        );
    }

    #[test]
    fn should_map_server_error_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::ServerError,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ServerError)
        );
    }

    #[test]
    fn should_map_invalid_library_status_to_invalid_url() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Invalid,
            WebsiteMetaInfo::None,
        );

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::InvalidUrl));
    }

    #[test]
    fn should_map_cloudflare_antibot_page() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            403,
            None,
            AntiBotTech::Cloudflare,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::CloudflareChallenge)
        );
    }

    #[test]
    fn should_map_non_cloudflare_antibot_page_to_bot_protection() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            403,
            None,
            AntiBotTech::DataDome,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::BotProtection)
        );
    }

    #[test]
    fn should_not_map_cloudflare_from_normal_page_without_library_antibot_signal() {
        let diagnostics = page_diagnostics("https://example.com/cloudflare-cdn", 200);

        assert_eq!(diagnostics.failure_kind, None);
    }

    #[test]
    fn should_ignore_antibot_vendor_detection_on_served_pages() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            200,
            None,
            AntiBotTech::Cloudflare,
        );

        assert_eq!(diagnostics.failure_kind, None);
    }

    #[test]
    fn should_map_library_final_redirect_to_other_domain() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            200,
            Some("https://other.com/"),
            AntiBotTech::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RedirectProblem)
        );
    }

    #[test]
    fn should_map_library_permanent_address_status_to_tls_error() {
        let diagnostics = page_diagnostics("https://example.com/", 526);

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::TlsError));
    }

    #[test]
    fn should_map_library_dns_status_to_connect_error() {
        let diagnostics = page_diagnostics("https://example.com/", 525);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ConnectError)
        );
    }

    #[test]
    fn should_map_library_too_many_redirects_status_to_redirect_problem() {
        let diagnostics = page_diagnostics("https://example.com/", 310);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RedirectProblem)
        );
    }
}
