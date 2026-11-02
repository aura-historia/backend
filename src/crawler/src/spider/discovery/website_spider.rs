use application::error::BoxError;
use bloomfilter::Bloom;
use regex::{Regex, RegexSet};
use reqwest::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};
use spider::CaseInsensitiveString;
use spider::page::{AntiBotTech, Page};
use spider::retry_strategy::{AttemptOutcome, RetryDirective, RetryStrategy};
use spider::tokio;
use spider::utils::auto_throttle::AutoThrottleConfig;
use spider::website::{CrawlStatus, Website, WebsiteMetaInfo};
use std::borrow::Cow;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use url::Url;

use crate::network::CRAWLER_USER_AGENT;
use crate::network::bot_challenge::{BotChallenge, ChallengeVendor, detect_bot_challenge};
use crate::network::policy::{
    NetworkAction, NetworkErrorKind, PublicTargetError, ReqwestErrorLike, RetryPolicy, action_for,
    classify_reqwest_error, inline_retry_backoff_for, is_same_or_www_host,
    network_error_kind_for_public_target_error, resolve_public_http_target,
};
use crate::spider::discovery::frontier::{
    CrawlDiscovery, CrawlRequest, FollowRules, FoundLinks, SeedAdmission,
};
use crate::spider::discovery::known_pages::{KnownPageAnswers, KnownPages};
use crate::spider::discovery::page_identity::page_identity_evidence;
use crate::spider::discovery::page_status::{RateLimitSignal, StatusCounts};
use crate::spider::discovery::robots::admit_crawl;
use crate::spider::discovery::root_host::{RootReroot, dominant_twin};
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::PageIdentityEvidence;

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

/// Single crawled page represented by its normalized URL and the identity it declares.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
    /// What the page's head says about its canonical URL and shop system; empty for a page
    /// whose body the crawl did not read.
    pub identity: PageIdentityEvidence,
}

impl CrawledPage {
    pub fn new(url: CrawledUrl) -> Self {
        Self {
            url,
            identity: PageIdentityEvidence::default(),
        }
    }

    pub fn with_identity(mut self, identity: PageIdentityEvidence) -> Self {
        self.identity = identity;
        self
    }
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
    /// Discovery was cut short before its bounds: the whole-crawl wall-clock
    /// budget ran out, or discovery stopped the crawl because its run ended.
    /// The pages delivered before it are valid but the crawl is incomplete.
    HardTimeoutTruncated,
    /// The shop answered a page with a rate limit and the spider stopped the
    /// crawl, carrying the answer and its `Retry-After`. The pages delivered
    /// before it are valid but the crawl is incomplete.
    RateLimited(RateLimitSignal),
}

impl CrawlCompletion {
    pub const fn as_str(self) -> &'static str {
        match self {
            CrawlCompletion::Completed => "COMPLETED",
            CrawlCompletion::PageBudgetReached => "PAGE_BUDGET_REACHED",
            CrawlCompletion::HardTimeoutTruncated => "HARD_TIMEOUT_TRUNCATED",
            CrawlCompletion::RateLimited(_) => "RATE_LIMITED",
        }
    }

    /// Whether discovery covered everything the configured bounds allow.
    pub const fn is_full(self) -> bool {
        matches!(
            self,
            CrawlCompletion::Completed | CrawlCompletion::PageBudgetReached
        )
    }

    /// The rate-limit answer that ended the crawl, when one did.
    pub const fn rate_limit(self) -> Option<RateLimitSignal> {
        match self {
            CrawlCompletion::RateLimited(signal) => Some(signal),
            _ => None,
        }
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

/// A crawl that left a frontier did not reach everything it found, whatever
/// else ended it: the link-discovery phase stopped at its share of the
/// budget, or Spider dropped seeds beyond its memory limit.
fn completion_with_frontier(completion: CrawlCompletion, frontier_left: bool) -> CrawlCompletion {
    match completion {
        CrawlCompletion::Completed if frontier_left => CrawlCompletion::HardTimeoutTruncated,
        completion => completion,
    }
}

/// Why discovery stopped Spider's crawl before Spider ended it on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlStopReason {
    /// The shop answered with a rate limit (`CrawlDiagnostics::rate_limit`).
    RateLimited,
    /// The consumer stopped taking pages: the spider run ended, typically by
    /// failing, before discovery finished.
    RunEnded,
}

impl CrawlStopReason {
    /// Stable label for logs; not persisted.
    pub const fn as_str(self) -> &'static str {
        match self {
            CrawlStopReason::RateLimited => "rate_limited",
            CrawlStopReason::RunEnded => "run_ended",
        }
    }
}

impl std::fmt::Display for CrawlStopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    /// ran.
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
    /// Pages refused by the URL blacklist, including links whose decoded form an admission
    /// rule refuses.
    pub rejected_blacklist: usize,
    pub rejected_duplicate: usize,
    /// How robots.txt applied to the crawl: `rules` or
    /// `unavailable_allow_all`.
    pub robots_policy: Option<&'static str>,
    pub robots_crawl_delay_ms: Option<u64>,
    /// Delay between request starts the crawl ran with: the configured delay,
    /// slowed after rate-limited crawls (`CrawlPace`) and raised by
    /// robots.txt `Crawl-delay`.
    pub request_delay_ms: u64,
    /// robots.txt Disallow rules whose written Allow exception Spider's deny
    /// list cannot carve out, so Spider skips URLs robots.txt permits.
    pub robots_rules_failing_closed: usize,
    /// Pages dropped because the local filter fell behind Spider's broadcast.
    pub lagged: u64,
    /// Delivered pages by the status class of their final answer.
    pub status_counts: StatusCounts,
    /// Delivered pages that were a bot challenge instead of the page asked for.
    pub bot_challenge_pages: usize,
    /// The rate-limit answer that stopped the crawl, with the shop's
    /// `Retry-After`; the pages delivered before it were kept.
    pub rate_limit: Option<RateLimitSignal>,
    /// Why discovery stopped Spider's crawl, when it did; unset when Spider
    /// ended the crawl itself.
    pub stopped_early: Option<CrawlStopReason>,
    /// Wall-clock budget the crawl ran with: the configured one, raised so
    /// that a robots.txt `Crawl-delay` still leaves room for an inference
    /// sample (`crawl_budget`).
    pub crawl_budget_ms: u64,
    /// Seed URLs carried over from the previous crawl, which the crawl visits
    /// after link discovery from the root.
    pub seeds: usize,
    /// Known product pages the delivered pages link that an incremental
    /// crawl did not fetch again, each page once.
    pub known_skipped: usize,
    /// Links to known pages an incremental crawl answered itself instead of
    /// requesting them, each link once: links Spider meets in a spelling
    /// it had not marked visited, such as with session or tracking
    /// parameters or under a Shopify collection
    /// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
    pub known_fetches_saved: usize,
    /// The move of the crawl root to its twin host the root page's links
    /// asked for, with the link counts (`docs/crawler/networking.md#destination-policy`).
    pub root_reroot: Option<RootReroot>,
}

impl CrawlDiagnostics {
    fn apply_signal(&mut self, signal: DiagnosticSignal) {
        self.failure_kind = Some(signal.kind);
        self.diagnostic_reason = Some(signal.reason.to_string());
    }

    fn count_rejection(&mut self, rejection: LocalRejection) {
        match rejection {
            LocalRejection::Host => self.rejected_host += 1,
            LocalRejection::Blacklist => self.rejected_blacklist += 1,
            LocalRejection::Duplicate => self.rejected_duplicate += 1,
        }
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

/// A running crawl. Dropping `pages` before it closes stops Spider's crawl,
/// which aborts its in-flight requests; `diagnostics` resolves only once
/// Spider's crawl has ended, so awaiting it after the last request is how a
/// consumer outlives the crawl it started.
#[derive(Debug)]
pub struct SpiderCrawl {
    pub pages: mpsc::Receiver<CrawledPage>,
    pub diagnostics: oneshot::Receiver<CrawlDiagnostics>,
    /// What the crawl left unvisited, once it has ended. A crawl that cannot
    /// tell drops the sender, and the caller keeps the frontier it had.
    pub discovery: oneshot::Receiver<CrawlDiscovery>,
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
    pub(super) fn preflight(kind: CrawlFailureKind, reason: &'static str) -> Self {
        Self::Preflight {
            kind,
            reason,
            source: None,
        }
    }

    /// A refused crawl-root redirect, carrying where the root redirected to.
    fn root_redirect(reason: &'static str, target: &Url) -> Self {
        Self::Preflight {
            kind: CrawlFailureKind::RedirectProblem,
            reason,
            source: Some(Box::new(RootRedirect::to(target))),
        }
    }

    /// Where the crawl root redirected to, when the preflight refused to follow it.
    pub fn redirect_target(&self) -> Option<&str> {
        match self {
            Self::Preflight {
                source: Some(source),
                ..
            } => source
                .downcast_ref::<RootRedirect>()
                .map(|redirect| redirect.target.as_str()),
            _ => None,
        }
    }
}

/// The target of a crawl-root redirect the preflight refused, in the spelling the crawler stores
/// (`docs/crawler/networking.md#stored-urls`): without credentials, fragment and the session or
/// tracking parameters that do not identify a page, so it can be logged.
#[derive(Debug, Error)]
#[error("the crawl root redirected to {target}")]
pub struct RootRedirect {
    target: String,
}

impl RootRedirect {
    fn to(target: &Url) -> Self {
        Self {
            target: CrawledUrl::new(target.clone()).to_string(),
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
            delay_millis: DEFAULT_DELAY_MILLIS,
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

/// Longest delay between request starts a slowed crawl uses; the same bound
/// Spider's adaptive throttle and robots.txt `Crawl-delay` are held to.
const MAX_CRAWL_DELAY_MILLIS: u64 = 60_000;

/// The request rate of one crawl. A domain whose crawls a shop rate-limited
/// is crawled more slowly: each step of its rate-limit streak doubles the
/// delay between request starts, from at least the default delay, up to
/// [`MAX_CRAWL_DELAY_MILLIS`] (`docs/crawler/scheduling-and-retries.md#rate-limited-crawls`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrawlPace {
    rate_limited_crawls: u32,
}

impl CrawlPace {
    /// The pace at a rate-limit streak of `rate_limited_crawls`; zero is the
    /// configured pace.
    pub const fn after_rate_limited_crawls(rate_limited_crawls: u32) -> Self {
        Self {
            rate_limited_crawls,
        }
    }

    /// Delay between request starts for a crawl configured with
    /// `configured_millis`.
    pub fn delay_millis(self, configured_millis: u64) -> u64 {
        if self.rate_limited_crawls == 0 {
            return configured_millis;
        }
        let base = configured_millis.max(DEFAULT_DELAY_MILLIS);
        let factor = 1_u64 << self.rate_limited_crawls.min(16);
        base.saturating_mul(factor)
            .min(MAX_CRAWL_DELAY_MILLIS.max(base))
    }
}

const DEFAULT_DELAY_MILLIS: u64 = 500;

/// Pages a crawl must have time for although robots.txt `Crawl-delay` paces
/// it: the minimum URL-pattern inference sample, with room for pages the local
/// filter drops.
const MIN_PAGES_WITHIN_CRAWL_DELAY: u32 = 40;

/// Longest wall-clock budget a `Crawl-delay` can raise a crawl to.
const MAX_DELAY_LIMITED_CRAWL_DURATION: Duration = Duration::from_secs(60 * 60);

/// The wall-clock budget of a crawl: the configured one, raised for a shop
/// whose robots.txt `Crawl-delay` would leave it fewer than
/// [`MIN_PAGES_WITHIN_CRAWL_DELAY`] pages, up to
/// [`MAX_DELAY_LIMITED_CRAWL_DURATION`]. The delay between requests stays as
/// the shop asked; only the crawl lasts longer. A pace slowed by rate limits
/// does not raise it: that shop asked for fewer requests, not slower ones.
fn crawl_budget(configured: Duration, crawl_delay: Option<Duration>) -> Duration {
    let Some(crawl_delay) = crawl_delay else {
        return configured;
    };
    let needed = crawl_delay.saturating_mul(MIN_PAGES_WITHIN_CRAWL_DELAY);
    configured.max(needed.min(MAX_DELAY_LIMITED_CRAWL_DURATION))
}

/// A crawl with seeds gives link discovery from the root at most this share
/// (one over the value) of its budget before it turns to the seeds, so neither
/// starves the other; link discovery not finished by then continues among the
/// seeds.
const LINK_DISCOVERY_BUDGET_DIVISOR: u32 = 2;

#[async_trait::async_trait]
#[mockall::automock]
pub trait Spider: Send + Sync {
    async fn crawl(&self, request: CrawlRequest) -> Result<SpiderCrawl, SpiderDiscoveryError>;
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

/// The page-body limit Spider reads from `SPIDER_MAX_SIZE_BYTES`, which the preflight holds the
/// root page it reads to as well.
fn configured_spider_body_limit(
    configured: Option<&str>,
    maximum: usize,
) -> Result<usize, SpiderDiscoveryError> {
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
    Ok(configured)
}

pub(super) fn configured_host_whitelist(url: &Url) -> Result<String, SpiderDiscoveryError> {
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
        return Err(SpiderDiscoveryError::root_redirect(
            "crawl-root redirect target is outside the configured bare/www host",
            &redirect_target,
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
    /// The move to the twin host the root page's links asked for, when it did.
    reroot: Option<RootReroot>,
}

/// Bounds the whole redirect chain, retries included, so a stalling host
/// cannot hold a spider slot for longer than `deadline`.
async fn preflight_crawl_root(
    transport: &dyn SpiderTransport,
    configured_root: Url,
    timeout: Duration,
    retry: RetryPolicy,
    deadline: Duration,
    max_body_bytes: usize,
) -> Result<PreflightedRoot, SpiderDiscoveryError> {
    tokio::time::timeout(
        deadline,
        follow_crawl_root(transport, configured_root, timeout, retry, max_body_bytes),
    )
    .await
    .unwrap_or_else(|_| {
        Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::ConnectError,
            "crawl-root preflight exceeded its time budget",
        ))
    })
}

/// Follows the crawl root's redirects within its bare/www host, and moves it to its twin host
/// once when the root page's links point mostly there ([`dominant_twin`]), the way a redirect to
/// the twin would; a move counts against the redirect bound. A host the chain already left is
/// never moved back to, and a move the twin redirects back from is not one.
async fn follow_crawl_root(
    transport: &dyn SpiderTransport,
    configured_root: Url,
    timeout: Duration,
    retry: RetryPolicy,
    max_body_bytes: usize,
) -> Result<PreflightedRoot, SpiderDiscoveryError> {
    let mut current_root = configured_root.clone();
    let mut visited_hosts = Vec::new();
    let mut reroot = None;

    for redirect_count in 0..=MAX_ROOT_REDIRECTS {
        let RootFetch { response, client } =
            fetch_crawl_root(transport, &current_root, timeout, retry).await?;
        visited_hosts.push(current_root.host_str().map(str::to_owned));

        if !response.status().is_redirection() {
            let status = response.status().as_u16();
            if reroot.is_none()
                && response.status().is_success()
                && redirect_count < MAX_ROOT_REDIRECTS
                && let Some(moved) = root_page_reroot(&current_root, response, max_body_bytes)
                    .await
                    .filter(|moved| {
                        !visited_hosts.contains(&moved.to.host_str().map(str::to_owned))
                    })
            {
                current_root = moved.to.clone();
                reroot = Some(moved);
                continue;
            }
            // A twin that redirected back did not take the crawl.
            let reroot = reroot.filter(|moved| moved.to.host_str() == current_root.host_str());
            return Ok(PreflightedRoot {
                url: current_root,
                status,
                client,
                reroot,
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

    Err(SpiderDiscoveryError::root_redirect(
        "crawl-root redirect limit exceeded",
        &current_root,
    ))
}

/// The move [`dominant_twin`] asks for, read from at most `max_body_bytes` of the root page.
/// A body that cannot be read asks for none: Spider fetches the root page again anyway.
async fn root_page_reroot(
    root: &Url,
    mut response: spider::reqwest::Response,
    max_body_bytes: usize,
) -> Option<RootReroot> {
    let mut body = Vec::new();
    while body.len() < max_body_bytes {
        match response.chunk().await {
            Ok(Some(chunk)) => body.extend_from_slice(&chunk),
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    body.truncate(max_body_bytes);
    dominant_twin(root, &body)
}

/// Bounded Spider retries that never re-fetch what a retry cannot improve: a
/// body the transport ceiling already truncated would truncate again, and a
/// 429 limits the client, not the page, so the crawl reacts to it instead
/// (see `RateLimitSignal`).
struct PageRetryStrategy {
    max_retries: u32,
}

impl RetryStrategy for PageRetryStrategy {
    fn max_retries(&self) -> u32 {
        self.max_retries
    }

    fn on_retry(&self, outcome: &AttemptOutcome) -> RetryDirective {
        if outcome.content_truncated || outcome.status_code.as_u16() == 429 {
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
    /// The filter task stopped the crawl before Spider ended it.
    stopped: bool,
    status: CrawlStatus,
    meta: WebsiteMetaInfo,
    initial_html_length: usize,
    initial_should_retry: bool,
    initial_anti_bot: AntiBotTech,
}

/// The whole-crawl bounds `crawl_completion` judges a finished crawl against.
#[derive(Debug, Clone, Copy)]
struct CrawlBounds {
    max_crawl_duration: Duration,
    max_pages_per_crawl: u32,
    /// The share of `max_crawl_duration` link discovery from the root takes
    /// before the crawl turns to its seeds.
    link_discovery_duration: Duration,
    /// Links marked visited before the crawl, which Spider counts as pages.
    known_links: usize,
}

/// Runs Spider to its end, or until the filter task asks it to stop, and
/// snapshots what the library learned about the site. Spider's broadcast
/// closes with the unsubscribe, which ends the filter task's page loop.
///
/// The crawl runs in two phases on one `Website`, so the second skips every
/// link the first visited. Link discovery from the root runs first, for its
/// share of the budget when there are seeds. The seeds, with the links the
/// first phase found but did not reach, follow for the rest of the budget,
/// unless the first phase failed at the root or was stopped. The filter task
/// tells, once it has relayed every page, what both phases left unvisited.
/// Spider reports a root answered with a challenge as idle, like a healthy
/// one, so the seed phase waits for the filter task's judgement of the root.
///
/// A stop drops the crawl future, which aborts Spider's in-flight page
/// fetches: Spider runs them in a `JoinSet` the crawl future owns, the same
/// way its own hard timeout ends a crawl. `Website::stop` cannot do this; it
/// needs the `&mut Website` the running crawl borrows and only takes effect
/// between links. The pages already broadcast still reach the filter task.
async fn run_library_crawl(
    mut website: Website,
    link_decoder: FoundLinkDecoder,
    bounds: CrawlBounds,
    seeds: Vec<String>,
    found: FoundLinks,
    stop: Arc<Notify>,
    mut root_judgement: RootJudgement,
) -> LibraryOutcome {
    let started = tokio::time::Instant::now();
    website.with_on_link_find_callback(Some(Arc::new(move |link, html| {
        (link_decoder.decode(link), html)
    })));
    website.with_crawl_timeout(Some(bounds.link_discovery_duration));
    let mut stopped = crawl_until_stopped(&mut website, &stop).await;
    let root_outcome = library_outcome(&website);

    let pages_visited = |website: &Website| website.size().saturating_sub(bounds.known_links);
    let remaining = || bounds.max_crawl_duration.saturating_sub(started.elapsed());
    let seed_phase_allowed = !stopped
        && root_outcome.status == CrawlStatus::Idle
        && pages_visited(&website) < bounds.max_pages_per_crawl as usize
        && !remaining().is_zero();
    if seed_phase_allowed {
        match judged_root(&mut root_judgement, &stop, remaining()).await {
            None => stopped = true,
            Some(RootVerdict::Refused) => {}
            Some(RootVerdict::Accepted) => {
                // Pages the filter task has not relayed yet miss here; what their
                // links lead to carries over in the frontier instead.
                let visited = website.get_links();
                let pending = found.unvisited(&seeds, |link| visited.contains(link));
                let remaining = remaining();
                if !pending.is_empty() && !remaining.is_zero() {
                    website.persist_links();
                    website.set_extra_links(pending.into_iter().map(Into::into).collect());
                    website.with_crawl_timeout(Some(remaining));
                    // Spider gives every crawl it runs the whole page limit again.
                    website.with_limit(seed_phase_page_limit(
                        bounds.max_pages_per_crawl,
                        pages_visited(&website),
                    ));
                    stopped = crawl_until_stopped(&mut website, &stop).await;
                }
            }
        }
    }

    let completion = if stopped {
        CrawlCompletion::HardTimeoutTruncated
    } else {
        crawl_completion(
            started.elapsed(),
            bounds.max_crawl_duration,
            pages_visited(&website),
            bounds.max_pages_per_crawl,
        )
    };
    website.unsubscribe();
    LibraryOutcome {
        completion,
        stopped,
        ..root_outcome
    }
}

/// The page limit that leaves the seed phase the pages link discovery did not
/// use of `max_pages`. Spider's limit counts the root page, which the seed
/// phase does not fetch again, so it allows one page fewer than it says.
fn seed_phase_page_limit(max_pages: u32, pages_visited: usize) -> u32 {
    let used = u32::try_from(pages_visited).unwrap_or(u32::MAX);
    max_pages.saturating_sub(used).saturating_add(1)
}

/// The filter task's judgement of the root page, unset until it relayed the
/// first page Spider delivered.
type RootJudgement = watch::Receiver<Option<RootVerdict>>;

/// Whether a crawl may go on from its root page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootVerdict {
    Accepted,
    /// The root failed the crawl, refused it or answered a bot challenge:
    /// nothing else the crawl fetches can help.
    Refused,
}

/// Waits, for at most `within`, until the filter task has judged the root
/// page; `None` when the filter task asked the crawl to stop meanwhile. A
/// root it did not judge in time, or could not judge, counts as refused.
async fn judged_root(
    judgement: &mut RootJudgement,
    stop: &Notify,
    within: Duration,
) -> Option<RootVerdict> {
    let judged = tokio::time::timeout(within, judgement.wait_for(Option::is_some));
    let stop = stop.notified();
    tokio::pin!(judged, stop);
    tokio::select! {
        biased;
        () = &mut stop => None,
        judged = &mut judged => Some(match judged {
            Ok(Ok(verdict)) => verdict.unwrap_or(RootVerdict::Refused),
            Ok(Err(_)) | Err(_) => RootVerdict::Refused,
        }),
    }
}

/// Runs one Spider crawl to its end; true when the filter task stopped it.
async fn crawl_until_stopped(website: &mut Website, stop: &Notify) -> bool {
    let crawl = website.crawl();
    let stop = stop.notified();
    tokio::pin!(crawl, stop);
    tokio::select! {
        biased;
        () = &mut crawl => false,
        () = &mut stop => true,
    }
}

/// What Spider learned about the site from its root; the completion is
/// judged once the whole crawl has ended.
fn library_outcome(website: &Website) -> LibraryOutcome {
    LibraryOutcome {
        completion: CrawlCompletion::default(),
        stopped: false,
        status: *website.get_status(),
        meta: *website.get_website_meta_info(),
        initial_html_length: website.get_initial_html_length(),
        initial_should_retry: website.get_initial_page_should_retry(),
        initial_anti_bot: *website.get_initial_anti_bot_tech(),
    }
}

/// Why the local filter dropped a page Spider delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalRejection {
    Host,
    Blacklist,
    Duplicate,
}

/// Local admission of the pages Spider delivers: the configured host only,
/// nothing the URL blacklist or the link decoder refused, each page once. The
/// two trailing-slash spellings of a page are one page, except that the slashed
/// spelling is still admitted after the unslashed one, because storage keeps a
/// page under its slashed spelling and moves its row there
/// (`docs/crawler/networking.md#stored-urls`).
struct LocalFilter {
    configured_root: Url,
    /// An unrelated root redirect makes every delivered page off-host.
    root_redirect_rejected: bool,
    refused_links: RefusedLinks,
    /// Spider's deny list, so a page's canonical link counts only when the crawl may fetch it.
    deny_list: Arc<RegexSet>,
    seen: Bloom<String>,
    /// The links on every delivered page, for the crawl's frontier.
    found_links: FoundLinks,
    /// The pages an incremental crawl knows, and the links to them it answered itself.
    known_pages: Arc<KnownPageAnswers>,
}

impl LocalFilter {
    fn admit(&mut self, raw_url: &str) -> Result<CrawledUrl, LocalRejection> {
        if is_refused_link(&self.refused_links, raw_url) {
            return Err(LocalRejection::Blacklist);
        }
        let parsed = Url::parse(raw_url).map_err(|_| LocalRejection::Host)?;
        let normalized = CrawledUrl::new(parsed);
        if self.root_redirect_rejected
            || !is_same_or_www_host(&self.configured_root, normalized.as_url())
        {
            return Err(LocalRejection::Host);
        }
        if normalized.is_blacklisted() {
            return Err(LocalRejection::Blacklist);
        }
        if self.seen.check(&admission_key(&normalized)) {
            return Err(LocalRejection::Duplicate);
        }
        let page = normalized.page_spelling().to_string();
        self.seen.set(&page);
        if normalized.as_url().as_str() != page {
            self.seen.set(&normalized.to_string());
        }
        Ok(normalized)
    }

    /// The identity `page`, admitted as `url`, declares: links the crawl may fetch only.
    fn identity_of(&self, url: &CrawledUrl, page: &Page) -> PageIdentityEvidence {
        let Ok(final_url) = Url::parse(page.get_url_final()) else {
            return PageIdentityEvidence::default();
        };
        page_identity_evidence(url, &final_url, page.get_html_bytes_u8(), |link| {
            !link.is_blacklisted() && !self.deny_list.is_match(link.as_url().as_str())
        })
    }
}

/// What a page must not have been seen under to be admitted: an unslashed URL
/// its page in either spelling, a slashed URL that same slashed spelling.
fn admission_key(url: &CrawledUrl) -> String {
    let page = url.page_spelling();
    if url.as_url() == &page {
        page.to_string()
    } else {
        url.to_string()
    }
}

/// robots.txt `Crawl-delay` only ever raises the configured spider delay.
fn crawl_delay_millis(configured_millis: u64, crawl_delay: Option<Duration>) -> u64 {
    crawl_delay.map_or(configured_millis, |delay| {
        configured_millis.max(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX))
    })
}

/// Hands Spider each link it finds in the spelling the crawler stores
/// (`docs/crawler/networking.md#stored-urls`), so Spider requests the stored URL.
///
/// It decodes HTML character references first. Spider reads `href` values raw, so
/// `?c=&amp;id=113` would otherwise be requested with an `amp;id` parameter and answered with the
/// wrong page. Spider has already resolved the link against the page URL, so this fixes
/// references in the query and path text, not references that encode URL structure. It then
/// drops the query parameters that do not identify the page, such as `utm_*` or a session id.
///
/// Spider admits a link (host whitelist, deny list, robots.txt) on that raw text before it calls
/// the decoder, so the rewritten URL is only handed back when it passes the same rules.
/// Otherwise Spider fetches the raw link it admitted, and the filter task drops it.
struct FoundLinkDecoder {
    host_whitelist: Regex,
    /// Spider's URL deny list: [`CrawledUrl::spider_deny_patterns`] and the robots.txt rules.
    deny_list: Arc<RegexSet>,
    refused: RefusedLinks,
}

/// Raw links whose rewritten form the decoder refused, shared with the filter task.
type RefusedLinks = Arc<Mutex<HashSet<String>>>;

fn is_refused_link(refused: &RefusedLinks, url: &str) -> bool {
    refused
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(url)
}

impl FoundLinkDecoder {
    fn decode(&self, link: CaseInsensitiveString) -> CaseInsensitiveString {
        let decoded = html_escape::decode_html_entities(link.inner());
        let entities_decoded = matches!(decoded, Cow::Owned(_));
        // Judge the URL in the form reqwest requests.
        let stored = match Url::parse(&decoded) {
            Ok(url) => {
                let stored = CrawledUrl::new(url.clone());
                if !entities_decoded && stored.as_url() == &url {
                    return link;
                }
                Some(stored)
            }
            // Spider fails such a link itself; only a link that decoding broke is refused.
            Err(_) if !entities_decoded => return link,
            Err(_) => None,
        };
        match stored {
            Some(stored) if self.admits(stored.as_url().as_str()) => {
                String::from(stored.as_url().clone()).into()
            }
            _ => {
                self.refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(link.inner().to_string());
                link
            }
        }
    }

    fn admits(&self, url: &str) -> bool {
        self.host_whitelist.is_match(url) && !self.deny_list.is_match(url)
    }
}

impl SpiderImpl {
    async fn crawl_with_body_limit(
        &self,
        request: CrawlRequest,
        configured_body_limit: Option<&str>,
    ) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        let max_body_bytes = configured_spider_body_limit(
            configured_body_limit,
            self.config.max_response_body_bytes,
        )?;
        let CrawlRequest {
            crawl_root_url,
            pace,
            seeds,
        } = request;
        let (tx, rx) = mpsc::channel(self.config.channel_size);
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
        let (discovery_tx, discovery_rx) = oneshot::channel();

        let configured_root = Url::parse(&crawl_root_url).map_err(|_| {
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
            reroot: root_reroot,
        } = preflight_crawl_root(
            self.transport.as_ref(),
            configured_root,
            request_timeout,
            self.config.preflight_retry,
            self.config.preflight_deadline,
            max_body_bytes,
        )
        .await?;
        let host_whitelist = configured_host_whitelist(&root_url)?;
        let host_whitelist_regex = Regex::new(&host_whitelist).map_err(|_| {
            SpiderDiscoveryError::Config("Failed to compile host whitelist".to_string())
        })?;
        let robots =
            admit_crawl(self.transport.as_ref(), &root_url, &client, request_timeout).await?;
        let delay_millis = crawl_delay_millis(
            pace.delay_millis(self.config.delay_millis),
            robots.crawl_delay,
        );
        let refused_links = RefusedLinks::default();
        let decoder_refused_links = Arc::clone(&refused_links);
        let mut website = Website::new(root_url.as_str());
        website.set_http_client(client.clone());

        let mut blacklist_regex = CrawledUrl::spider_deny_patterns();
        blacklist_regex.extend(
            robots
                .deny_list
                .patterns
                .iter()
                .map(|pattern| pattern.into()),
        );
        // Spider silently drops a deny list that fails to compile, which
        // would drop robots.txt with it.
        let deny_list = RegexSet::new(&blacklist_regex).map(Arc::new).map_err(|_| {
            SpiderDiscoveryError::preflight(
                CrawlFailureKind::RobotsBlocked,
                "robots.txt rules do not compile into Spider's deny list",
            )
        })?;
        let crawl_duration = crawl_budget(self.config.max_crawl_duration, robots.crawl_delay);
        let follow_rules = Arc::new(FollowRules::new(
            root_url.clone(),
            host_whitelist_regex.clone(),
            Arc::clone(&deny_list),
        ));
        let admission = SeedAdmission {
            rules: &follow_rules,
            robots: &robots,
        };
        let seed_urls = admission.admit_all(&seeds.frontier);
        let link_discovery_duration = match seed_urls.is_empty() {
            true => crawl_duration,
            false => crawl_duration / LINK_DISCOVERY_BUDGET_DIVISOR,
        };
        let auto_throttle_config = AutoThrottleConfig {
            min_delay_ms: delay_millis,
            max_delay_ms: 60_000,
            ..AutoThrottleConfig::default()
        };
        website
            .configuration
            .with_auto_throttle(auto_throttle_config);
        website
            .configuration
            .with_concurrency_limit(Some(self.config.concurrency_limit.max(1)));

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_whitelist_url(Some(vec![host_whitelist.into()]))
            .with_respect_robots_txt(false)
            .with_return_page_links(true)
            .with_headers(Some(spider_request_headers()))
            .with_user_agent(Some(CRAWLER_USER_AGENT))
            .with_request_timeout(Some(request_timeout))
            .with_crawl_timeout(Some(link_discovery_duration))
            .with_limit(self.config.max_pages_per_crawl)
            .with_delay(delay_millis)
            .with_caching(false)
            .with_retry_strategy(Arc::new(PageRetryStrategy {
                max_retries: u32::from(self.config.retry_count),
            }))
            .build()
            .map_err(|_| SpiderDiscoveryError::Config("Failed to build website".to_string()))?;
        let known_pages = Arc::new(KnownPageAnswers::new(KnownPages::new(&root_url, &seeds)));
        let known_link_count = mark_known_links_visited(&mut website, known_pages.known()).await;
        if !known_pages.known().is_empty() {
            website.with_shared_fetch_engine(known_pages.clone());
        }

        let spider_rx = website.subscribe(512);

        let found_links = FoundLinks::new(follow_rules);
        let link_decoder = FoundLinkDecoder {
            host_whitelist: host_whitelist_regex,
            deny_list: Arc::clone(&deny_list),
            refused: decoder_refused_links,
        };
        let bounds = CrawlBounds {
            max_crawl_duration: crawl_duration,
            max_pages_per_crawl: self.config.max_pages_per_crawl,
            link_discovery_duration,
            known_links: known_link_count,
        };
        let stop_crawl = Arc::new(Notify::new());
        let crawl_stop = Arc::clone(&stop_crawl);
        let (root_verdict, root_judgement) = watch::channel(None);
        let seed_count = seed_urls.len();
        let crawl_found_links = found_links.clone();
        let frontier_seeds = seed_urls.clone();
        tokio::spawn(async move {
            let outcome = run_library_crawl(
                website,
                link_decoder,
                bounds,
                seed_urls,
                crawl_found_links,
                crawl_stop,
                root_judgement,
            )
            .await;
            let _ = outcome_tx.send(outcome);
        });

        let crawl_root_url = root_url.to_string();
        let filter = LocalFilter {
            configured_root: root_url,
            root_redirect_rejected: false,
            refused_links,
            deny_list,
            seen: Bloom::new_for_fp_rate(self.config.bloom_capacity, self.config.bloom_fp_rate)
                .expect("bloom filter init failed"),
            found_links: found_links.clone(),
            known_pages: Arc::clone(&known_pages),
        };
        let mut diagnostics = CrawlDiagnostics {
            root_preflight_status: Some(root_preflight_status),
            robots_policy: Some(robots.policy),
            robots_crawl_delay_ms: robots
                .crawl_delay
                .map(|delay| u64::try_from(delay.as_millis()).unwrap_or(u64::MAX)),
            robots_rules_failing_closed: robots.deny_list.failing_closed.len(),
            request_delay_ms: delay_millis,
            crawl_budget_ms: u64::try_from(crawl_duration.as_millis()).unwrap_or(u64::MAX),
            seeds: seed_count,
            root_reroot,
            ..CrawlDiagnostics::default()
        };
        tokio::spawn(async move {
            let stop_requested = relay_library_pages(
                spider_rx,
                &tx,
                filter,
                &mut diagnostics,
                &crawl_root_url,
                &stop_crawl,
                &root_verdict,
            )
            .await;
            // Spider's crawl has ended once its outcome arrives, so the
            // diagnostics never reach the consumer while it still fetches.
            match outcome_rx.await {
                Ok(outcome) => {
                    if outcome.stopped {
                        diagnostics.stopped_early = stop_requested;
                    }
                    // Every page is relayed by now, so its links are known.
                    let discovery = CrawlDiscovery {
                        frontier: found_links.frontier(&frontier_seeds, known_pages.known()),
                    };
                    diagnostics.known_skipped = found_links.skipped_known(known_pages.known());
                    diagnostics.known_fetches_saved = known_pages.answered_links();
                    apply_library_outcome(&mut diagnostics, outcome);
                    diagnostics.completion = completion_with_frontier(
                        diagnostics.completion,
                        !discovery.frontier.is_empty(),
                    );
                    let _ = discovery_tx.send(discovery);
                }
                // The crawl task died without reporting; never call that full.
                Err(_) => {
                    diagnostics.completion =
                        rate_limited_or(&diagnostics, CrawlCompletion::HardTimeoutTruncated);
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
            discovery: discovery_rx,
        })
    }
}

/// Marks the known links visited so Spider skips them. Spider keeps marked
/// links only when the crawl continues an earlier state, so the website is
/// marked as such. Returns how many links Spider now counts as visited, which
/// its page count includes.
async fn mark_known_links_visited(website: &mut Website, known: &KnownPages) -> usize {
    if known.is_empty() {
        return 0;
    }
    website.persist_links();
    for link in known.spellings() {
        website.insert_link(link).await;
    }
    website.size()
}

/// Relays the pages Spider broadcasts through the local filter to the
/// consumer until the broadcast closes and every accepted page is delivered,
/// or the consumer is gone. A rate-limit answer, or the consumer going away
/// while Spider still crawls, asks the crawl task to stop; the first reason
/// asked for is returned.
async fn relay_library_pages(
    mut spider_rx: broadcast::Receiver<Page>,
    tx: &mpsc::Sender<CrawledPage>,
    mut filter: LocalFilter,
    diagnostics: &mut CrawlDiagnostics,
    crawl_root_url: &str,
    stop_crawl: &Notify,
    root_verdict: &watch::Sender<Option<RootVerdict>>,
) -> Option<CrawlStopReason> {
    let mut stop_requested = None;
    let mut request_stop = |reason| {
        stop_requested.get_or_insert(reason);
        stop_crawl.notify_one();
    };
    let mut first_page_seen = false;
    // Accepted URLs wait here instead of blocking the broadcast
    // subscription: Spider's 512-slot broadcast drops pages once the
    // subscriber falls behind, while this queue only holds small
    // normalized URLs and is bounded by the crawl page budget.
    let mut pending: VecDeque<CrawledPage> = VecDeque::new();
    let mut broadcast_open = true;

    while broadcast_open || !pending.is_empty() {
        let page = ::tokio::select! {
            biased;
            // The consumer is gone; nothing left to deliver, and every
            // further fetch would be wasted load on the shop.
            () = tx.closed() => {
                if broadcast_open {
                    request_stop(CrawlStopReason::RunEnded);
                }
                break;
            }
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
                // Closed: the next turn's first branch stops the crawl.
                Err(_) => continue,
            },
        };
        if filter.known_pages.answered(page.get_url()) {
            continue;
        }
        diagnostics.pages_received += 1;
        let status_code = page.status_code.as_u16();
        diagnostics.status_counts.count(status_code);
        let challenge = page_bot_challenge(status_code, &page);
        if challenge.is_some() {
            diagnostics.bot_challenge_pages += 1;
        }

        if !first_page_seen {
            apply_library_page(
                diagnostics,
                crawl_root_url,
                page.get_url(),
                status_code,
                page.final_redirect_destination.as_deref(),
                RootRefusal {
                    anti_bot_tech: page.anti_bot_tech,
                    challenge,
                },
            );
            filter.root_redirect_rejected = matches!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::RedirectProblem)
            );
            // Seeds cannot help a crawl whose root failed or refused it.
            let refused =
                diagnostics.failure_kind.is_some() || is_access_denied_status(status_code);
            root_verdict.send_replace(Some(match refused {
                true => RootVerdict::Refused,
                false => RootVerdict::Accepted,
            }));
            first_page_seen = true;
        }

        let rate_limit_answer = RateLimitSignal::from_page(status_code, page.headers.as_ref());
        if diagnostics.rate_limit.is_none()
            && let Some(signal) = rate_limit_answer
        {
            apply_rate_limit_signal(diagnostics, signal);
            request_stop(CrawlStopReason::RateLimited);
        }
        // A challenge or a rate-limit answer is not the page: it stays in the
        // frontier for the next crawl, and neither its links nor its head are
        // the shop's.
        let is_page = challenge.is_none() && rate_limit_answer.is_none();
        if is_page {
            filter.found_links.record_delivered(page.get_url());
            if let Some(page_links) = page.page_links.as_deref() {
                filter.found_links.record(page.get_url(), page_links);
            }
        }

        match filter.admit(page.get_url()) {
            Ok(url) => {
                diagnostics.accepted += 1;
                let identity = if is_page {
                    filter.identity_of(&url, &page)
                } else {
                    PageIdentityEvidence::default()
                };
                pending.push_back(CrawledPage::new(url).with_identity(identity));
            }
            Err(rejection) => diagnostics.count_rejection(rejection),
        }
    }
    stop_requested
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(&self, request: CrawlRequest) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        let configured_body_limit = std::env::var(SPIDER_MAX_BODY_BYTES_ENV).ok();
        self.crawl_with_body_limit(request, configured_body_limit.as_deref())
            .await
    }
}

/// The bot challenge a delivered page is instead of the page it was asked for, if it provably
/// is one.
fn page_bot_challenge(status_code: u16, page: &Page) -> Option<BotChallenge> {
    let no_headers = HeaderMap::new();
    detect_bot_challenge(
        status_code,
        page.headers.as_ref().unwrap_or(&no_headers),
        page.get_html_bytes_u8(),
    )
}

/// What says the shop refused the root page: Spider's reading of the vendor
/// in front of it, and a bot challenge it answered instead of the page.
#[derive(Debug, Clone, Copy)]
struct RootRefusal {
    anti_bot_tech: AntiBotTech,
    challenge: Option<BotChallenge>,
}

/// Records the root page Spider delivered: its status, where it ended up and
/// any failure signal the library or the challenge detector attached to it.
fn apply_library_page(
    diagnostics: &mut CrawlDiagnostics,
    crawl_root_url: &str,
    page_url: &str,
    status_code: u16,
    final_redirect_destination: Option<&str>,
    refusal: RootRefusal,
) {
    let final_url = final_redirect_destination.unwrap_or(page_url);
    diagnostics.http_status = Some(status_code);
    diagnostics.redirect_url = final_redirect_destination
        .filter(|destination| *destination != page_url)
        .and_then(loggable_url);

    if let Some(signal) = page_diagnostic_signal(crawl_root_url, final_url, status_code, refusal) {
        diagnostics.apply_signal(signal);
    }
    diagnostics.final_url = loggable_url(final_url);
}

/// `url` in the spelling the crawler stores (`docs/crawler/networking.md#stored-urls`), so a
/// diagnostic never carries credentials or session parameters; none when it does not parse.
fn loggable_url(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .map(|url| CrawledUrl::new(url).to_string())
}

fn page_diagnostic_signal(
    crawl_root_url: &str,
    final_url: &str,
    status_code: u16,
    refusal: RootRefusal,
) -> Option<DiagnosticSignal> {
    anti_bot_signal(status_code, refusal.anti_bot_tech)
        .or_else(|| refusal.challenge.map(bot_challenge_signal))
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

/// A root answered with a bot challenge, a success status included, is refused
/// rather than a tiny crawl.
fn bot_challenge_signal(challenge: BotChallenge) -> DiagnosticSignal {
    match challenge.vendor {
        ChallengeVendor::Cloudflare => DiagnosticSignal::new(
            CrawlFailureKind::CloudflareChallenge,
            "bot_challenge_root_page",
        ),
        ChallengeVendor::AwsWaf
        | ChallengeVendor::DataDome
        | ChallengeVendor::SiteGround
        | ChallengeVendor::Unattributed => {
            DiagnosticSignal::new(CrawlFailureKind::BotProtection, "bot_challenge_root_page")
        }
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

/// Records the answer that stops the crawl. Spider's reading of a refused
/// root (a vendor block, a JavaScript challenge) keeps precedence over the
/// generic rate-limit failure, like it does over the access-denied one.
fn apply_rate_limit_signal(diagnostics: &mut CrawlDiagnostics, signal: RateLimitSignal) {
    diagnostics.rate_limit = Some(signal);
    if diagnostics.failure_kind.is_none() {
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::RateLimited,
            RATE_LIMIT_ANSWER_STOPPED_CRAWL,
        ));
    }
}

const RATE_LIMIT_ANSWER_STOPPED_CRAWL: &str = "rate_limit_answer_stopped_crawl";

/// A rate-limit answer ends the crawl as rate-limited however Spider stopped
/// afterwards: the answered page and the links behind it were not crawled.
fn rate_limited_or(diagnostics: &CrawlDiagnostics, completion: CrawlCompletion) -> CrawlCompletion {
    diagnostics
        .rate_limit
        .map_or(completion, CrawlCompletion::RateLimited)
}

fn apply_library_outcome(diagnostics: &mut CrawlDiagnostics, outcome: LibraryOutcome) {
    diagnostics.completion = rate_limited_or(diagnostics, outcome.completion);
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

/// Distinguishes "the site refused the crawler" and "Spider delivered pages
/// but local filtering accepted none" from a genuinely empty library
/// response; a robots.txt refusal of the root fails the preflight instead. Library signals keep precedence, so a 403 Spider
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
    use crate::network::robots::RobotsRules;
    use crate::spider::discovery::frontier::CrawlSeeds;
    use crate::spider::discovery::root_host;
    use crate::spider::utils::url::canonical::{ShopPlatform, canonical_product_url};
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
            RootRefusal {
                anti_bot_tech,
                challenge: None,
            },
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
        /// `Html` answered after a delay, like a slow server.
        SlowHtml(Duration, &'static str, String),
        /// An empty answer with extra header lines, each `Name: value\r\n`.
        WithHeaders(&'static str, &'static str),
        /// `Html` with extra header lines, each `Name: value\r\n`.
        HtmlWithHeaders(&'static str, &'static str, String),
        Redirect(&'static str),
        /// Close the connection without answering, like a crashed server.
        Drop,
        /// Accept the request and never answer, like a stalled server.
        Hang,
    }

    /// `(host, target, user_agent, accept_encoding)` per request.
    type RequestLog = Arc<tokio::sync::Mutex<Vec<(String, String, String, String)>>>;
    type SiteHandler = Arc<dyn Fn(&str, usize) -> SiteResponse + Send + Sync>;
    /// A handler that answers by the request's `Host` as well: `(host, target, previous)`.
    type HostSiteHandler = Arc<dyn Fn(&str, &str, usize) -> SiteResponse + Send + Sync>;

    struct TestSite {
        addr: SocketAddr,
        requests: RequestLog,
        server: tokio::task::JoinHandle<()>,
    }

    impl TestSite {
        async fn spawn(handler: SiteHandler) -> Self {
            Self::spawn_for_hosts(Arc::new(move |_, target, previous| {
                handler(target, previous)
            }))
            .await
        }

        async fn spawn_for_hosts(handler: HostSiteHandler) -> Self {
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
        handler: HostSiteHandler,
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
                    log.push((host.clone(), target.clone(), user_agent, accept_encoding));
                    previous
                };
                let html_response = |status: &str, extra_headers: &str, body: &str| {
                    format!(
                        "HTTP/1.1 {status}\r\n{extra_headers}Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let response = match handler(&host, &target, previous) {
                    SiteResponse::Html(status, body) => html_response(status, "", &body),
                    SiteResponse::HtmlWithHeaders(status, extra_headers, body) => {
                        html_response(status, extra_headers, &body)
                    }
                    SiteResponse::SlowHtml(delay, status, body) => {
                        tokio::time::sleep(delay).await;
                        html_response(status, "", &body)
                    }
                    SiteResponse::WithHeaders(status, extra_headers) => format!(
                        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
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
            ..
        } = spider
            .crawl_with_body_limit(
                CrawlRequest::from_root(root, CrawlPace::default()),
                Some("8388608"),
            )
            .await?;
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

    fn canonical_link_site(target: &str, _previous: usize) -> SiteResponse {
        match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/collections/all/products/ladle">Ladle</a>
                   <a href="/products/old">Moved</a>"#
                    .to_string(),
            ),
            "/collections/all/products/ladle" => SiteResponse::Html(
                "200 OK",
                r#"<!doctype html><html><head>
                   <script>Shopify.shop = "shop.myshopify.com";</script>
                   <link rel="canonical" href="/products/ladle">
                   </head><body></body></html>"#
                    .to_string(),
            ),
            "/products/old" => SiteResponse::Redirect("/products/new"),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }
    }

    /// Each delivered page carries what its own head declares. Spider does not follow a redirect,
    /// so a redirected URL is delivered without the target's head and declares nothing.
    #[tokio::test]
    async fn should_deliver_each_page_with_the_identity_its_head_declares() {
        let site = TestSite::spawn(Arc::new(canonical_link_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let SpiderCrawl { mut pages, .. } = spider
            .crawl_with_body_limit(
                CrawlRequest::from_root("http://example.com/", CrawlPace::default()),
                Some("8388608"),
            )
            .await
            .unwrap();
        let mut identities = std::collections::HashMap::new();
        while let Some(page) = pages.recv().await {
            identities.insert(page.url.to_string(), page.identity);
        }
        site.finish().await;

        let collection_page = &identities["http://example.com/collections/all/products/ladle"];
        assert_eq!(
            collection_page
                .canonical_link
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("http://example.com/products/ladle")
        );
        assert_eq!(collection_page.platform, Some(ShopPlatform::Shopify));
        assert_eq!(
            identities.get("http://example.com/products/old"),
            Some(&PageIdentityEvidence::default())
        );
    }

    /// The crawl's robots.txt decides whether a collection page names its Shopify product URL:
    /// where robots.txt disallows `/products/`, no rule moves the page, which is stored as
    /// fetched (`docs/crawler/networking.md#canonical-product-urls`).
    #[tokio::test]
    async fn should_apply_the_shopify_rule_only_where_robots_txt_allows_the_product_url() {
        let pattern = regex::Regex::new(r"/products/[^/]+$").unwrap();
        for (robots, expected) in [
            (
                "User-agent: *\nDisallow: /cart\n",
                Some("http://example.com/products/ladle"),
            ),
            ("User-agent: *\nDisallow: /products/\n", None),
        ] {
            // The collection page names no canonical link, so only the Shopify rule can move it.
            let site = TestSite::spawn(Arc::new(move |target: &str, _| match target {
                "/robots.txt" => SiteResponse::Html("200 OK", robots.to_string()),
                "/" => SiteResponse::Html(
                    "200 OK",
                    r#"<a href="/collections/all/products/ladle">Ladle</a>"#.to_string(),
                ),
                "/collections/all/products/ladle" => SiteResponse::Html(
                    "200 OK",
                    r#"<!doctype html><html><head>
                       <script>Shopify.shop = "shop.myshopify.com";</script>
                       </head><body></body></html>"#
                        .to_string(),
                ),
                _ => SiteResponse::Html("404 Not Found", String::new()),
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let SpiderCrawl { mut pages, .. } = spider
                .crawl_with_body_limit(
                    CrawlRequest::from_root("http://example.com/", CrawlPace::default()),
                    Some("8388608"),
                )
                .await
                .unwrap();
            let mut collection_page = None;
            while let Some(page) = pages.recv().await {
                if page.url.as_url().path() == "/collections/all/products/ladle" {
                    collection_page = Some(page);
                }
            }
            site.finish().await;

            let page = collection_page.expect("collection page delivered");
            assert_eq!(
                canonical_product_url(
                    &page.url,
                    &page.identity,
                    Some(ShopPlatform::Shopify),
                    &pattern
                )
                .map(|url| url.to_string())
                .as_deref(),
                expected,
                "{robots}"
            );
            assert_eq!(
                page.identity.platform_product_urls.is_empty(),
                expected.is_none(),
                "{robots}"
            );
        }
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
                    <a href="/shop?page=9&amp;utm_source=mail">Tracked page</a>
                    <a href="/index.php?category=26245&amp;page=3">Category page</a>
                    <a href="/literal?x=1&y=2">Literal ampersand</a>
                    <a href="/encoded?q=a%26b">Percent-encoded ampersand</a>
                    <a href="/private&sol;page">Robots-disallowed once decoded</a>
                    <a href="/post?p=1&amp;replytocom=55">Blacklisted once decoded</a>
                    <a href="/double?cat=5&amp;amp;page=2">Double-encoded</a>"#
                    .to_string(),
            ),
            _ => SiteResponse::Html("200 OK", "<p>page</p>".to_string()),
        }
    }

    #[tokio::test]
    async fn should_fetch_and_emit_entity_decoded_urls_when_hrefs_carry_html_entities() {
        let site = TestSite::spawn(Arc::new(entity_encoded_href_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let request_log = site.finish().await;

        let expected = [
            "http://example.com/single.asp?c=&offset=20&id=113",
            "http://example.com/shop?page=9",
            "http://example.com/index.php?category=26245&page=3",
            "http://example.com/literal?x=1&y=2",
            "http://example.com/encoded?q=a%26b",
            // Decoded once, as a browser does; the shop answers the same URL.
            "http://example.com/double?cat=5&amp;page=2",
        ];
        for url in expected {
            assert!(discovered.contains(url), "missing {url} in {discovered:?}");
        }
        // The shop must receive the decoded query, or it renders the wrong page
        // and every link discovered from it inherits the mistake.
        for target in [
            "/single.asp?c=&offset=20&id=113",
            "/shop?page=9",
            "/index.php?category=26245&page=3",
        ] {
            assert!(
                request_log
                    .iter()
                    .any(|(_, requested, ..)| requested == target),
                "missing request for {target}: {request_log:?}"
            );
        }
        // Spider follows neither a presentation parameter nor an action, also behind `&amp;`,
        // so the currency and reply-to links cost no request.
        assert!(
            !request_log.iter().any(|(_, target, ..)| target
                .contains("currency")
                || target.contains("replytocom")),
            "presentation or action link fetched: {request_log:?}"
        );
        // Decoding never turns a link Spider admitted into one that robots.txt or the
        // blacklist refuses.
        assert!(
            !request_log
                .iter()
                .any(|(_, target, ..)| target.starts_with("/private/")
                    || target.contains("&replytocom=")),
            "decoded URL bypassed robots.txt or the blacklist: {request_log:?}"
        );
        // Spider still fetches the raw links the decoder refused, but they are never emitted.
        assert!(
            !discovered
                .iter()
                .any(|url| url.contains("replytocom") || url.contains("sol;")),
            "refused link emitted: {discovered:?}"
        );
        assert_eq!(diagnostics.rejected_blacklist, 1, "{diagnostics:?}");
    }

    fn link_decoder_with_robots(robots_txt: &str) -> FoundLinkDecoder {
        let root = Url::parse("https://www.example.com/").unwrap();
        let mut deny_list: Vec<String> = CrawledUrl::spider_deny_patterns()
            .into_iter()
            .map(String::from)
            .collect();
        deny_list.extend(RobotsRules::parse(robots_txt).deny_list().patterns);
        FoundLinkDecoder {
            host_whitelist: Regex::new(&configured_host_whitelist(&root).unwrap()).unwrap(),
            deny_list: Arc::new(RegexSet::new(deny_list).unwrap()),
            refused: RefusedLinks::default(),
        }
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
                "https://www.example.com/shop?page=8",
            ),
            (
                "https://www.example.com/index.php?category=26245&amp;page=3",
                "https://www.example.com/index.php?category=26245&page=3",
            ),
            // The decoded URL is returned in the form reqwest requests.
            (
                "https://www.example.com/caf&eacute;?a=1&amp;b=2",
                "https://www.example.com/caf%C3%A9?a=1&b=2",
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
    fn link_decoder_should_drop_non_identity_parameters_when_link_has_no_character_reference() {
        let decoder = link_decoder_with_robots("");

        for (link, expected) in [
            (
                "https://www.example.com/products/1?utm_source=mail&fbclid=x",
                "https://www.example.com/products/1",
            ),
            (
                "https://www.example.com/products/netsuke?variant=4211",
                "https://www.example.com/products/netsuke",
            ),
            (
                "https://www.example.com/catalog/product_info.php?products_id=7&osCsid=a1b2",
                "https://www.example.com/catalog/product_info.php?products_id=7",
            ),
            (
                "https://www.example.com/products/1?id=2",
                "https://www.example.com/products/1?id=2",
            ),
        ] {
            assert_eq!(decode_link(&decoder, link), expected);
            assert!(!is_refused_link(&decoder.refused, link), "{link}");
        }
    }

    #[test]
    fn link_decoder_should_keep_raw_link_when_decoded_url_breaks_an_admission_rule() {
        let decoder =
            link_decoder_with_robots("User-agent: *\nDisallow: /private/\nDisallow: /admin\n");

        for link in [
            // robots.txt, which Spider matches on the path only
            "https://www.example.com/private&sol;page",
            // URL blacklist
            "https://www.example.com/post?p=1&amp;replytocom=55",
            // judged on the requested form, where `\..\` collapses to `/admin`
            "https://www.example.com/a&bsol;..&bsol;admin",
        ] {
            assert_eq!(decode_link(&decoder, link), link);
            assert!(
                is_refused_link(&decoder.refused, link),
                "{link} not recorded"
            );
        }
    }

    #[test]
    fn link_decoder_should_decode_backslash_reference_like_a_literal_backslash() {
        let decoder = link_decoder_with_robots("");
        let link = "https://www.example.com/a&bsol;..&bsol;admin";

        assert_eq!(decode_link(&decoder, link), "https://www.example.com/admin");
        assert!(!is_refused_link(&decoder.refused, link));
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
        assert_eq!(
            completion_with_frontier(CrawlCompletion::Completed, true),
            CrawlCompletion::HardTimeoutTruncated
        );
        assert_eq!(
            completion_with_frontier(CrawlCompletion::PageBudgetReached, true),
            CrawlCompletion::PageBudgetReached
        );
        assert_eq!(
            completion_with_frontier(CrawlCompletion::Completed, false),
            CrawlCompletion::Completed
        );
    }

    #[test]
    fn crawl_completion_should_use_stable_persisted_identifiers() {
        assert_eq!(
            [
                CrawlCompletion::Completed,
                CrawlCompletion::PageBudgetReached,
                CrawlCompletion::HardTimeoutTruncated,
                CrawlCompletion::RateLimited(RateLimitSignal {
                    status: 429,
                    retry_after: None,
                }),
            ]
            .map(CrawlCompletion::as_str),
            [
                "COMPLETED",
                "PAGE_BUDGET_REACHED",
                "HARD_TIMEOUT_TRUNCATED",
                "RATE_LIMITED"
            ]
        );
        assert_eq!(
            serde_json::to_string(&CrawlCompletion::HardTimeoutTruncated).unwrap(),
            "\"HARD_TIMEOUT_TRUNCATED\""
        );
        assert!(CrawlCompletion::Completed.is_full());
        assert!(CrawlCompletion::PageBudgetReached.is_full());
        assert!(!CrawlCompletion::HardTimeoutTruncated.is_full());
        assert!(
            !CrawlCompletion::RateLimited(RateLimitSignal {
                status: 503,
                retry_after: Some(Duration::from_secs(60)),
            })
            .is_full()
        );
    }

    #[test]
    fn crawl_pace_should_double_the_delay_per_rate_limited_crawl_up_to_one_minute() {
        let pace = CrawlPace::after_rate_limited_crawls;

        assert_eq!(CrawlPace::default().delay_millis(500), 500);
        assert_eq!(pace(0).delay_millis(0), 0);
        assert_eq!(pace(1).delay_millis(500), 1_000);
        assert_eq!(pace(2).delay_millis(500), 2_000);
        assert_eq!(
            pace(1).delay_millis(0),
            1_000,
            "slows from the default delay"
        );
        assert_eq!(pace(1).delay_millis(5_000), 10_000);
        assert_eq!(pace(7).delay_millis(500), MAX_CRAWL_DELAY_MILLIS);
        assert_eq!(pace(u32::MAX).delay_millis(500), MAX_CRAWL_DELAY_MILLIS);
        assert_eq!(
            pace(3).delay_millis(90_000),
            90_000,
            "never faster than configured"
        );
    }

    mod bot_challenge_pages {
        use super::*;

        const CHALLENGE: &str =
            "<html><head><title>Just a moment...</title></head><body></body></html>";

        #[tokio::test]
        async fn should_fail_a_crawl_whose_root_answers_a_challenge_with_200_as_that_challenge() {
            let site = TestSite::spawn(Arc::new(|_: &str, _: usize| {
                SiteResponse::Html("200 OK", CHALLENGE.to_string())
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let (_, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
            site.finish().await;

            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::CloudflareChallenge),
                "{diagnostics:?}"
            );
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some("bot_challenge_root_page")
            );
            assert_eq!(diagnostics.bot_challenge_pages, 1);
        }

        /// A vendor's challenge header reaches the detector through the page Spider delivers, so a
        /// `202` interstitial is recognised by it even when its markup names no vendor.
        #[tokio::test]
        async fn should_fail_a_crawl_whose_root_answers_a_202_interstitial_by_its_header() {
            let site = TestSite::spawn(Arc::new(|_: &str, _: usize| {
                SiteResponse::HtmlWithHeaders(
                    "202 Accepted",
                    "sg-captcha: challenge\r\n",
                    "<html><head><title>shop.example</title></head><body><p>Please wait.</p></body></html>"
                        .to_string(),
                )
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let (_, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
            site.finish().await;

            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::BotProtection),
                "{diagnostics:?}"
            );
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some("bot_challenge_root_page")
            );
            assert_eq!(diagnostics.http_status, Some(202));
        }

        /// Spider reports a crawl whose root answered `202` idle, like a healthy one; only the
        /// filter task recognises the challenge, so the seed phase waits for its judgement.
        #[tokio::test]
        async fn should_fetch_no_seed_after_a_challenged_root() {
            let site = TestSite::spawn(Arc::new(|_: &str, _: usize| {
                SiteResponse::HtmlWithHeaders(
                    "202 Accepted",
                    "sg-captcha: challenge\r\n",
                    "<html><body><p>Please wait.</p></body></html>".to_string(),
                )
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
            let seeds = CrawlSeeds {
                frontier: (1..=5).map(|i| example(&format!("/seed/{i}"))).collect(),
                ..CrawlSeeds::default()
            };

            let (_, diagnostics, _) = run_seeded_crawl(&spider, seeds).await;
            let requests = site.finish().await;

            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::BotProtection),
                "{diagnostics:?}"
            );
            assert!(
                !requests
                    .iter()
                    .any(|(_, target, ..)| target.starts_with("/seed/")),
                "{requests:?}"
            );
        }

        #[tokio::test]
        async fn should_count_a_challenged_listing_page_without_failing_an_ordinary_root() {
            let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
                "/" => SiteResponse::Html(
                    "200 OK",
                    r#"<html><head><title>Antiques</title></head><body><a href="/chairs">Chairs</a></body></html>"#
                        .to_string(),
                ),
                _ => SiteResponse::Html("200 OK", CHALLENGE.to_string()),
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let (_, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
            site.finish().await;

            assert_eq!(diagnostics.failure_kind, None, "{diagnostics:?}");
            assert_eq!(diagnostics.bot_challenge_pages, 1);
        }

        /// A challenge is not the page, so what its head declares never canonicalizes the URL.
        #[tokio::test]
        async fn should_deliver_a_challenged_page_without_the_identity_its_head_declares() {
            let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
                "/" => SiteResponse::Html(
                    "200 OK",
                    r#"<html><head><title>Antiques</title></head><body><a href="/en/chair">Chair</a></body></html>"#
                        .to_string(),
                ),
                _ => SiteResponse::Html(
                    "200 OK",
                    r#"<html><head><title>Just a moment...</title><link rel="canonical" href="/chair"></head><body></body></html>"#
                        .to_string(),
                ),
            }))
            .await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let SpiderCrawl {
                mut pages,
                diagnostics,
                ..
            } = spider
                .crawl_with_body_limit(
                    CrawlRequest::from_root("http://example.com/", CrawlPace::default()),
                    Some("8388608"),
                )
                .await
                .unwrap();
            let mut identities = std::collections::HashMap::new();
            while let Some(page) = pages.recv().await {
                identities.insert(page.url.to_string(), page.identity);
            }
            site.finish().await;

            assert_eq!(diagnostics.await.unwrap().bot_challenge_pages, 1);
            assert_eq!(
                identities.get("http://example.com/en/chair"),
                Some(&PageIdentityEvidence::default())
            );
        }
    }

    #[tokio::test]
    async fn should_crawl_at_the_slowed_pace_after_rate_limited_crawls() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/a">a</a><a href="/b">b</a>"#.to_string(),
            ),
            _ => SiteResponse::Html("200 OK", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let started = tokio::time::Instant::now();

        let SpiderCrawl {
            mut pages,
            diagnostics,
            ..
        } = spider
            .crawl_with_body_limit(
                CrawlRequest::from_root(
                    "http://example.com/",
                    CrawlPace::after_rate_limited_crawls(1),
                ),
                Some("8388608"),
            )
            .await
            .unwrap();
        let mut delivered = 0;
        while pages.recv().await.is_some() {
            delivered += 1;
        }
        let diagnostics = diagnostics.await.unwrap();
        let elapsed = started.elapsed();
        site.finish().await;

        assert_eq!(delivered, 3, "{diagnostics:?}");
        assert_eq!(diagnostics.request_delay_ms, 1_000);
        assert!(
            elapsed >= Duration::from_millis(900),
            "the links behind the root wait for the slowed delay: {elapsed:?}"
        );
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

    async fn run_seeded_crawl(
        spider: &SpiderImpl,
        seeds: CrawlSeeds,
    ) -> (HashSet<String>, CrawlDiagnostics, CrawlDiscovery) {
        let SpiderCrawl {
            mut pages,
            diagnostics,
            discovery,
        } = spider
            .crawl_with_body_limit(
                CrawlRequest {
                    seeds,
                    ..CrawlRequest::from_root("http://example.com/", CrawlPace::default())
                },
                Some("8388608"),
            )
            .await
            .unwrap();
        let mut delivered = HashSet::new();
        while let Some(page) = pages.recv().await {
            delivered.insert(page.url.to_string());
        }
        (
            delivered,
            diagnostics.await.unwrap(),
            discovery.await.unwrap(),
        )
    }

    fn example(path: &str) -> Url {
        Url::parse(&format!("http://example.com{path}")).unwrap()
    }

    /// The root links two products and a hanging page; `/deep/1` is only
    /// known from an earlier crawl's frontier and links `/deep/2`.
    fn carried_over_site(target: &str, _: usize) -> SiteResponse {
        match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/product/1">1</a><a href="/product/2">2</a>"#.to_string(),
            ),
            "/deep/1" => SiteResponse::Html("200 OK", r#"<a href="/deep/2">2</a>"#.to_string()),
            "/product/1" | "/product/2" | "/deep/2" => {
                SiteResponse::Html("200 OK", "<p>p</p>".to_string())
            }
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }
    }

    #[tokio::test]
    async fn should_continue_from_the_carried_over_frontier_after_link_discovery() {
        let site = TestSite::spawn(Arc::new(carried_over_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            frontier: vec![
                example("/deep/1"),
                example("/product/1"),
                Url::parse("http://other.example/deep/3").unwrap(),
                example("/cart"),
            ],
            ..CrawlSeeds::default()
        };

        let (delivered, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        for expected in [
            "http://example.com/",
            "http://example.com/product/1",
            "http://example.com/product/2",
            "http://example.com/deep/1",
            "http://example.com/deep/2",
        ] {
            assert!(delivered.contains(expected), "missing {expected}");
        }
        // Off-host and blacklisted seeds are never handed to Spider.
        assert_eq!(diagnostics.seeds, 2, "{diagnostics:?}");
        assert!(!requests.iter().any(|(_, target, ..)| target == "/cart"));
        // A page both seeded and linked is fetched once.
        assert_eq!(
            requests
                .iter()
                .filter(|(_, target, ..)| target == "/product/1")
                .count(),
            1
        );
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    #[tokio::test]
    async fn should_leave_what_a_cut_short_crawl_did_not_visit_as_its_frontier() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/slow">slow</a><a href="/product/1">1</a>"#.to_string(),
            ),
            "/slow" => SiteResponse::Hang,
            "/product/1" => {
                SiteResponse::Html("200 OK", r#"<a href="/product/2">2</a>"#.to_string())
            }
            _ => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
        }))
        .await;
        let config = CrawlerConfig {
            request_timeout_secs: 5,
            concurrency_limit: 1,
            delay_millis: 300,
            max_crawl_duration: Duration::from_millis(1_200),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            frontier: (1..=20).map(|i| example(&format!("/seed/{i}"))).collect(),
            ..CrawlSeeds::default()
        };

        let (delivered, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        site.finish().await;

        assert_eq!(
            diagnostics.completion,
            CrawlCompletion::HardTimeoutTruncated
        );
        assert!(!discovery.frontier.is_empty());
        for left in &discovery.frontier {
            assert!(
                !delivered.contains(left.as_str()),
                "{left} was visited but carried over"
            );
        }
        assert!(
            discovery
                .frontier
                .iter()
                .any(|url| url.path().starts_with("/seed/")),
            "unvisited seeds carry over: {discovery:?}"
        );
    }

    #[tokio::test]
    async fn should_carry_over_the_links_a_crawl_without_seeds_found_but_did_not_reach() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                (1..=30)
                    .map(|i| format!(r#"<a href="/item/{i}">{i}</a>"#))
                    .collect(),
            ),
            _ => {
                SiteResponse::SlowHtml(Duration::from_millis(200), "200 OK", "<p>p</p>".to_string())
            }
        }))
        .await;
        let config = CrawlerConfig {
            max_pages_per_crawl: 100,
            max_crawl_duration: Duration::from_millis(1_500),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));

        let (delivered, diagnostics, discovery) =
            run_seeded_crawl(&spider, CrawlSeeds::default()).await;
        site.finish().await;

        assert_eq!(
            diagnostics.completion,
            CrawlCompletion::HardTimeoutTruncated
        );
        assert!(delivered.len() < 31, "{delivered:?}");
        assert_eq!(
            discovery.frontier.len(),
            31 - delivered.len(),
            "{discovery:?}"
        );
    }

    /// Spider fetches the crawl root's host alone and every link under the root's scheme, so a
    /// link to the `www` twin or under the other scheme can never be visited and must not keep
    /// a crawl that reached everything else from completing.
    #[tokio::test]
    async fn should_complete_a_crawl_whose_pages_link_the_www_twin_or_the_other_scheme() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/product/1">1</a>
                   <a href="http://www.example.com/product/2">twin</a>
                   <a href="https://example.com/product/3">other scheme</a>"#
                    .to_string(),
            ),
            "/product/1" | "/product/3" => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "www.example.com"]),
        );

        let (delivered, diagnostics, discovery) =
            run_seeded_crawl(&spider, CrawlSeeds::default()).await;
        let requests = site.finish().await;

        assert!(delivered.contains("http://example.com/product/3"));
        assert!(
            !requests.iter().any(|(host, ..)| host == "www.example.com"),
            "{requests:?}"
        );
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    /// Spider gives every crawl it runs the whole page budget again, while the budget bounds the
    /// whole crawl, link discovery and seed phase together.
    #[tokio::test]
    async fn should_bound_link_discovery_and_seed_phase_together_by_the_page_budget() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html("200 OK", r#"<a href="/slow">slow</a>"#.to_string()),
            "/slow" => SiteResponse::Hang,
            _ => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
        }))
        .await;
        let config = CrawlerConfig {
            request_timeout_secs: 5,
            concurrency_limit: 2,
            max_pages_per_crawl: 4,
            max_crawl_duration: Duration::from_secs(2),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            frontier: (1..=10).map(|i| example(&format!("/seed/{i}"))).collect(),
            ..CrawlSeeds::default()
        };

        let (_, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        let seeds_fetched = requests
            .iter()
            .filter(|(_, target, ..)| target.starts_with("/seed/"))
            .count();
        // The root and the hanging page took two pages of the budget.
        assert_eq!(seeds_fetched, 2, "{requests:?}");
        assert_eq!(diagnostics.completion, CrawlCompletion::PageBudgetReached);
        assert_eq!(discovery.frontier.len(), 9, "{discovery:?}");
    }

    /// Spider counts the link as written visited, while the page arrives under the spelling the
    /// crawler stores, so the seed phase must not take the delivered page for an unvisited link.
    #[tokio::test]
    async fn should_fetch_a_page_linked_with_tracking_parameters_once() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/product/1?utm_source=newsletter">1</a>"#.to_string(),
            ),
            "/product/1" | "/deep/1" => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            frontier: vec![example("/deep/1")],
            ..CrawlSeeds::default()
        };

        let (delivered, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        assert!(delivered.contains("http://example.com/deep/1"));
        assert_eq!(
            requests
                .iter()
                .filter(|(_, target, ..)| target.starts_with("/product/1"))
                .count(),
            1,
            "{requests:?}"
        );
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    #[tokio::test]
    async fn should_keep_a_page_answered_with_a_rate_limit_in_the_frontier() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html("200 OK", r#"<a href="/limited">l</a>"#.to_string()),
            "/limited" => SiteResponse::WithHeaders("429 Too Many Requests", "Retry-After: 60\r\n"),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (_, diagnostics, discovery) = run_seeded_crawl(&spider, CrawlSeeds::default()).await;
        site.finish().await;

        assert!(
            matches!(diagnostics.completion, CrawlCompletion::RateLimited(_)),
            "{diagnostics:?}"
        );
        assert_eq!(discovery.frontier, [example("/limited")]);
    }

    #[tokio::test]
    async fn should_skip_known_urls_in_either_spelling_on_an_incremental_crawl() {
        let site = TestSite::spawn(Arc::new(carried_over_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            known: vec![example("/product/1/")],
            ..CrawlSeeds::default()
        };

        let (delivered, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        assert!(delivered.contains("http://example.com/product/2"));
        assert!(
            !requests
                .iter()
                .any(|(_, target, ..)| target == "/product/1")
        );
        assert_eq!(diagnostics.known_skipped, 1);
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    /// A URL retired as a duplicate is skipped like a known product URL but is no product page
    /// of its own, so it never counts as one the crawl found linked.
    #[tokio::test]
    async fn should_skip_retired_duplicates_without_counting_them_as_known_products() {
        let site = TestSite::spawn(Arc::new(carried_over_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            known: vec![example("/product/2")],
            retired: vec![example("/product/1/")],
            ..CrawlSeeds::default()
        };

        let (_, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        assert!(
            !requests
                .iter()
                .any(|(_, target, ..)| target.starts_with("/product/")),
            "{requests:?}"
        );
        assert_eq!(diagnostics.known_skipped, 1);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    /// A Shopify storefront whose root links two stored products, one under two collections and
    /// one with a session parameter, and a product the domain does not store yet.
    fn shopify_incremental_site(target: &str, _: usize) -> SiteResponse {
        match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<!doctype html><html><head>
                   <script>Shopify.shop = "shop.myshopify.com";</script>
                   </head><body>
                   <a href="/collections/chairs/products/oak-chair?variant=1">Oak</a>
                   <a href="/collections/sale/products/oak-chair">Oak on sale</a>
                   <a href="/products/pine-chair?sessionid=abc">Pine</a>
                   <a href="/products/elm-chair">Elm</a>
                   </body></html>"#
                    .to_string(),
            ),
            "/collections/chairs/products/oak-chair"
            | "/collections/sale/products/oak-chair"
            | "/products/pine-chair"
            | "/products/elm-chair" => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }
    }

    fn is_stored_chair(target: &str) -> bool {
        target.contains("oak-chair") || target.contains("pine-chair")
    }

    #[tokio::test]
    async fn should_not_fetch_known_products_linked_with_session_parameters_or_under_a_collection()
    {
        let full_crawl_site = TestSite::spawn(Arc::new(shopify_incremental_site)).await;
        let spider =
            SpiderImpl::with_transport(test_config(), full_crawl_site.transport(&["example.com"]));
        let (_, full_crawl, _) = run_seeded_crawl(&spider, CrawlSeeds::default()).await;
        let full_crawl_requests = full_crawl_site.finish().await;
        let stored_chair_requests = full_crawl_requests
            .iter()
            .filter(|(_, target, ..)| is_stored_chair(target))
            .count();
        assert_eq!(stored_chair_requests, 3, "{full_crawl_requests:?}");
        assert_eq!(full_crawl.known_fetches_saved, 0);

        let site = TestSite::spawn(Arc::new(shopify_incremental_site)).await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            known: vec![
                example("/products/oak-chair"),
                example("/products/pine-chair"),
            ],
            ..CrawlSeeds::default()
        };
        let (delivered, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        assert!(
            !requests
                .iter()
                .any(|(_, target, ..)| is_stored_chair(target)),
            "{requests:?}"
        );
        assert_eq!(
            delivered,
            HashSet::from([
                "http://example.com/".to_string(),
                "http://example.com/products/elm-chair".to_string(),
            ])
        );
        assert_eq!(diagnostics.known_fetches_saved, 3, "{diagnostics:?}");
        assert_eq!(diagnostics.known_skipped, 2, "{diagnostics:?}");
        assert_eq!(diagnostics.pages_received, 2, "{diagnostics:?}");
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
        assert!(discovery.frontier.is_empty(), "{discovery:?}");
    }

    /// swedishantiques.biz: the bare host serves a root page that links `twin_links` pages on
    /// `www.`, where the shop's pages live and link each other relatively.
    fn www_linked_site(twin_links: usize) -> HostSiteHandler {
        Arc::new(move |host, target, _| match (host, target) {
            ("example.com", "/") => SiteResponse::Html(
                "200 OK",
                (1..=twin_links)
                    .map(|i| format!(r#"<a href="https://www.example.com/category/{i}">c</a>"#))
                    .chain(std::iter::once(r#"<a href="/about">About</a>"#.to_string()))
                    .collect(),
            ),
            ("www.example.com", "/") => SiteResponse::Html(
                "200 OK",
                r#"<a href="/category/1">Chairs</a><a href="/product/1">Oak</a>"#.to_string(),
            ),
            ("www.example.com", "/category/1" | "/product/1") | ("example.com", "/about") => {
                SiteResponse::Html("200 OK", "<p>p</p>".to_string())
            }
            _ => SiteResponse::Html("404 Not Found", String::new()),
        })
    }

    #[tokio::test]
    async fn should_move_a_bare_root_whose_page_links_mostly_www_to_the_www_host() {
        let site = TestSite::spawn_for_hosts(www_linked_site(root_host::MIN_TWIN_LINKS)).await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "www.example.com"]),
        );

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let requests = site.finish().await;

        assert_eq!(
            discovered,
            HashSet::from([
                "http://www.example.com/".to_string(),
                "http://www.example.com/category/1".to_string(),
                "http://www.example.com/product/1".to_string(),
            ])
        );
        assert_eq!(
            diagnostics.root_reroot,
            Some(RootReroot {
                from: Url::parse("http://example.com/").unwrap(),
                to: Url::parse("http://www.example.com/").unwrap(),
                twin_links: root_host::MIN_TWIN_LINKS,
                root_links: 1,
            })
        );
        // The bare host is asked for its root page once, in preflight; robots.txt comes from
        // the host the crawl runs on.
        assert_eq!(
            requests
                .iter()
                .filter(|(host, ..)| host == "example.com")
                .map(|(_, target, ..)| target.as_str())
                .collect::<Vec<_>>(),
            ["/"]
        );
        assert!(
            requests
                .iter()
                .any(|(host, target, ..)| host == "www.example.com" && target == "/robots.txt")
        );
    }

    #[tokio::test]
    async fn should_keep_a_root_whose_page_links_the_twin_below_the_threshold() {
        let site = TestSite::spawn_for_hosts(www_linked_site(root_host::MIN_TWIN_LINKS - 1)).await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "www.example.com"]),
        );

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let requests = site.finish().await;

        assert_eq!(
            discovered,
            HashSet::from([
                "http://example.com/".to_string(),
                "http://example.com/about".to_string(),
            ])
        );
        assert_eq!(diagnostics.root_reroot, None);
        assert!(
            requests.iter().all(|(host, ..)| host == "example.com"),
            "{requests:?}"
        );
    }

    /// A root that redirects to `www.` whose page links the bare host throughout is not moved
    /// back there: the redirect named the host the shop serves.
    #[tokio::test]
    async fn should_not_move_the_root_back_to_the_host_it_redirected_from() {
        let site = TestSite::spawn_for_hosts(Arc::new(|host, target, _| match (host, target) {
            ("example.com", "/") => SiteResponse::Redirect("http://www.example.com/"),
            ("www.example.com", "/") => SiteResponse::Html(
                "200 OK",
                (1..=2 * root_host::MIN_TWIN_LINKS)
                    .map(|i| format!(r#"<a href="http://example.com/category/{i}">c</a>"#))
                    .collect(),
            ),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "www.example.com"]),
        );

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        site.finish().await;

        assert_eq!(
            discovered,
            HashSet::from(["http://www.example.com/".to_string()])
        );
        assert_eq!(diagnostics.root_reroot, None);
    }

    /// A bare root whose page links `www.` throughout, where `www.` redirects back to the bare
    /// host, as shops with stale absolute links do: the crawl stays on the bare host, and its
    /// diagnostics record no move.
    #[tokio::test]
    async fn should_record_no_move_when_the_twin_redirects_back() {
        let www_links = www_linked_site(2 * root_host::MIN_TWIN_LINKS);
        let site = TestSite::spawn_for_hosts(Arc::new(move |host, target, previous| {
            match (host, target) {
                ("www.example.com", "/") => SiteResponse::Redirect("http://example.com/"),
                _ => www_links(host, target, previous),
            }
        }))
        .await;
        let spider = SpiderImpl::with_transport(
            test_config(),
            site.transport(&["example.com", "www.example.com"]),
        );

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        site.finish().await;

        assert_eq!(
            discovered,
            HashSet::from([
                "http://example.com/".to_string(),
                "http://example.com/about".to_string(),
            ])
        );
        assert_eq!(diagnostics.root_reroot, None);
    }

    #[test]
    fn crawl_budget_should_leave_a_crawl_delay_limited_shop_room_for_the_inference_sample() {
        let configured = Duration::from_secs(600);

        assert_eq!(crawl_budget(configured, None), configured);
        assert_eq!(
            crawl_budget(configured, Some(Duration::from_secs(1))),
            configured
        );
        // antieksite.nl: `Crawl-delay: 30` left 20 pages in 600 s.
        assert_eq!(
            crawl_budget(configured, Some(Duration::from_secs(30))),
            Duration::from_secs(30 * u64::from(MIN_PAGES_WITHIN_CRAWL_DELAY))
        );
        assert_eq!(
            crawl_budget(configured, Some(Duration::from_secs(600))),
            MAX_DELAY_LIMITED_CRAWL_DURATION
        );
    }

    /// An endless chain of slow pages: only a stop ends the crawl before
    /// its wall-clock budget.
    fn endless_chain_site(target: &str, _: usize) -> SiteResponse {
        match target.strip_prefix("/chain/") {
            Some(hop) => {
                let hop: usize = hop.parse().unwrap_or(0);
                SiteResponse::SlowHtml(
                    Duration::from_millis(20),
                    "200 OK",
                    format!(r#"<a href="/chain/{}">next</a>"#, hop + 1),
                )
            }
            None if target == "/" => {
                SiteResponse::Html("200 OK", r#"<a href="/chain/1">chain</a>"#.to_string())
            }
            None => SiteResponse::Html("404 Not Found", String::new()),
        }
    }

    /// The root links a page the shop rate limits and the head of a slow,
    /// endless chain of pages, so a crawl that keeps going is visible.
    fn rate_limited_site(limited: SiteResponse) -> SiteHandler {
        Arc::new(move |target: &str, previous: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/limited">limited</a><a href="/chain/1">chain</a>"#.to_string(),
            ),
            "/limited" => limited.clone(),
            _ => endless_chain_site(target, previous),
        })
    }

    /// A shop that asked for less traffic during link discovery gets no seed phase: the seeds
    /// carry over to the crawl its rate-limit hold allows.
    #[tokio::test]
    async fn should_fetch_no_seed_after_a_rate_limit_answer() {
        let site = TestSite::spawn(rate_limited_site(SiteResponse::WithHeaders(
            "429 Too Many Requests",
            "Retry-After: 60\r\n",
        )))
        .await;
        let config = CrawlerConfig {
            max_pages_per_crawl: 1_000,
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));
        let seeds = CrawlSeeds {
            frontier: (1..=5).map(|i| example(&format!("/seed/{i}"))).collect(),
            ..CrawlSeeds::default()
        };

        let (_, diagnostics, discovery) = run_seeded_crawl(&spider, seeds).await;
        let requests = site.finish().await;

        assert!(
            matches!(diagnostics.completion, CrawlCompletion::RateLimited(_)),
            "{diagnostics:?}"
        );
        assert!(
            !requests
                .iter()
                .any(|(_, target, ..)| target.starts_with("/seed/")),
            "{requests:?}"
        );
        for kept in ["/limited", "/seed/1", "/seed/5"] {
            assert!(
                discovery.frontier.contains(&example(kept)),
                "{kept} carries over: {discovery:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_stop_the_crawl_on_a_rate_limit_answer_and_record_retry_after() {
        for (answer, retry_count, expected) in [
            (
                SiteResponse::WithHeaders("429 Too Many Requests", "Retry-After: 1800\r\n"),
                2,
                RateLimitSignal {
                    status: 429,
                    retry_after: Some(Duration::from_secs(1800)),
                },
            ),
            (
                SiteResponse::WithHeaders("503 Service Unavailable", "Retry-After: 120\r\n"),
                0,
                RateLimitSignal {
                    status: 503,
                    retry_after: Some(Duration::from_secs(120)),
                },
            ),
        ] {
            let site = TestSite::spawn(rate_limited_site(answer)).await;
            let config = CrawlerConfig {
                retry_count,
                max_pages_per_crawl: 1_000,
                ..test_config()
            };
            let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));

            let (discovered, diagnostics) =
                run_crawl(&spider, "http://example.com/").await.unwrap();

            assert_eq!(diagnostics.rate_limit, Some(expected), "{diagnostics:?}");
            assert_eq!(
                diagnostics.failure_kind,
                Some(CrawlFailureKind::RateLimited),
                "{diagnostics:?}"
            );
            assert_eq!(
                diagnostics.diagnostic_reason.as_deref(),
                Some(RATE_LIMIT_ANSWER_STOPPED_CRAWL)
            );
            assert_eq!(
                diagnostics.completion,
                CrawlCompletion::RateLimited(expected),
                "{diagnostics:?}"
            );
            assert_eq!(
                diagnostics.stopped_early,
                Some(CrawlStopReason::RateLimited),
                "{diagnostics:?}"
            );
            let counted = match expected.status {
                429 => diagnostics.status_counts.rate_limited,
                _ => diagnostics.status_counts.server_error,
            };
            assert_eq!(counted, 1, "{diagnostics:?}");
            assert!(
                discovered.contains("http://example.com/"),
                "pages delivered before the answer are kept: {discovered:?}"
            );
            // A 429 is not retried per page; the 503 case runs without retries.
            assert_eq!(
                site.requests_for("/limited").await,
                1,
                "{}",
                expected.status
            );
            // Stopped: the chain, which the hard timeout alone would walk for
            // seconds, ends within a few hops of the answer.
            assert_eq!(
                site.requests_for("/chain/50").await,
                0,
                "crawl went on after the {} answer",
                expected.status
            );
            site.finish().await;
        }
    }

    #[tokio::test]
    async fn should_stop_the_crawl_before_reporting_when_the_run_ends_early() {
        let site = TestSite::spawn(Arc::new(endless_chain_site)).await;
        let config = CrawlerConfig {
            max_pages_per_crawl: 1_000,
            max_crawl_duration: Duration::from_secs(60),
            ..test_config()
        };
        let spider = SpiderImpl::with_transport(config, site.transport(&["example.com"]));
        let SpiderCrawl {
            mut pages,
            diagnostics,
            ..
        } = spider
            .crawl_with_body_limit(
                CrawlRequest::from_root("http://example.com/", CrawlPace::default()),
                Some("8388608"),
            )
            .await
            .unwrap();
        for _ in 0..3 {
            pages.recv().await.expect("the chain delivers pages");
        }

        // The run returns early, as on a failed classification: it drops the
        // page channel and waits for the diagnostics before giving up its
        // domain lock.
        drop(pages);
        let diagnostics = tokio::time::timeout(Duration::from_secs(5), diagnostics)
            .await
            .expect("the crawl stops promptly, not at its wall-clock budget")
            .unwrap();
        // Requests written before the stop may still be on their way to the
        // site's request log.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let served = site.requests.lock().await.len();
        tokio::time::sleep(Duration::from_secs(1)).await;
        let log = site.finish().await;

        assert_eq!(
            log.len(),
            served,
            "requests after the run returned: {log:?}"
        );
        assert_eq!(
            diagnostics.stopped_early,
            Some(CrawlStopReason::RunEnded),
            "{diagnostics:?}"
        );
        assert_eq!(
            diagnostics.completion,
            CrawlCompletion::HardTimeoutTruncated,
            "{diagnostics:?}"
        );
    }

    #[tokio::test]
    async fn should_count_delivered_pages_by_status_class() {
        let site = TestSite::spawn(Arc::new(|target: &str, _: usize| match target {
            "/" => SiteResponse::Html(
                "200 OK",
                r#"<a href="/missing">missing</a><a href="/product/1">product</a>"#.to_string(),
            ),
            "/product/1" => SiteResponse::Html("200 OK", "<p>p</p>".to_string()),
            _ => SiteResponse::Html("404 Not Found", String::new()),
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (_, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        site.finish().await;

        assert_eq!(
            diagnostics.status_counts,
            StatusCounts {
                success: 2,
                client_error: 1,
                ..StatusCounts::default()
            },
            "{diagnostics:?}"
        );
        assert_eq!(diagnostics.pages_received, 3);
        assert_eq!(diagnostics.rate_limit, None);
        assert_eq!(diagnostics.failure_kind, None);
        assert_eq!(diagnostics.completion, CrawlCompletion::Completed);
    }

    #[test]
    fn should_keep_the_library_root_signal_over_the_rate_limit_failure() {
        let mut diagnostics = page_diagnostics("https://example.com/", 429);
        diagnostics.apply_signal(DiagnosticSignal::new(
            CrawlFailureKind::CloudflareChallenge,
            "library_cloudflare_antibot",
        ));
        let signal = RateLimitSignal {
            status: 429,
            retry_after: None,
        };

        apply_rate_limit_signal(&mut diagnostics, signal);

        assert_eq!(diagnostics.rate_limit, Some(signal));
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::CloudflareChallenge)
        );

        let mut diagnostics = page_diagnostics("https://example.com/", 429);
        apply_rate_limit_signal(&mut diagnostics, signal);
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RateLimited)
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some(RATE_LIMIT_ANSWER_STOPPED_CRAWL)
        );
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
                SiteResponse::Redirect(
                    "https://user:secret@other.example/landing?sessionid=abc&ref=7#top",
                )
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
        // Logged as the crawler would store it: no credentials, session id or fragment.
        assert_eq!(
            error.redirect_target(),
            Some("https://other.example/landing?ref=7")
        );
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
        assert_eq!(
            error.redirect_target(),
            Some(format!("http://example.com/hop{MAX_ROOT_REDIRECTS}").as_str())
        );
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

    fn site_with_robots(robots: SiteResponse) -> SiteHandler {
        Arc::new(move |target: &str, previous: usize| {
            if target == "/robots.txt" {
                robots.clone()
            } else {
                regression_site(target, previous)
            }
        })
    }

    #[tokio::test]
    async fn should_never_request_links_robots_txt_disallows() {
        let site = TestSite::spawn(site_with_robots(SiteResponse::Html(
            "200 OK",
            "User-agent: *\nDisallow: /product/\n".to_string(),
        )))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(!discovered.contains("http://example.com/product/1"));
        assert!(discovered.contains("http://example.com/category/1"));
        assert_eq!(diagnostics.robots_policy, Some("rules"));
        assert_eq!(site.requests_for("/product/1").await, 0);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_apply_the_group_naming_the_crawler_over_the_wildcard_group() {
        let site = TestSite::spawn(site_with_robots(SiteResponse::Html(
            "200 OK",
            "User-agent: *\nAllow: /\n\nUser-agent: AuraHistoriaBot\nDisallow: /product\n"
                .to_string(),
        )))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (discovered, _) = run_crawl(&spider, "http://example.com/").await.unwrap();

        assert!(discovered.contains("http://example.com/category/1"));
        assert_eq!(site.requests_for("/product/1").await, 0);
        site.finish().await;
    }

    #[tokio::test]
    async fn should_refuse_the_crawl_when_robots_txt_disallows_the_root_or_is_unreachable() {
        for (robots, expected_kind) in [
            (
                SiteResponse::Html("200 OK", "User-agent: *\nDisallow: /\n".to_string()),
                CrawlFailureKind::RobotsBlocked,
            ),
            (
                SiteResponse::Html("503 Service Unavailable", String::new()),
                CrawlFailureKind::ServerError,
            ),
        ] {
            let site = TestSite::spawn(site_with_robots(robots)).await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let error = run_crawl(&spider, "http://example.com/").await.unwrap_err();

            assert!(
                matches!(error, SpiderDiscoveryError::Preflight { kind, .. } if kind == expected_kind),
                "{error:?}"
            );
            assert_eq!(site.requests_for("/category/1").await, 0);
            site.finish().await;
        }
    }

    #[tokio::test]
    async fn should_crawl_everything_when_robots_txt_answers_4xx() {
        for status in ["404 Not Found", "403 Forbidden"] {
            let site =
                TestSite::spawn(site_with_robots(SiteResponse::Html(status, String::new()))).await;
            let spider =
                SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

            let (discovered, diagnostics) =
                run_crawl(&spider, "http://example.com/").await.unwrap();

            assert!(
                discovered.contains("http://example.com/product/1"),
                "{status}"
            );
            assert_eq!(diagnostics.robots_policy, Some("unavailable_allow_all"));
            site.finish().await;
        }
    }

    fn local_filter() -> LocalFilter {
        LocalFilter {
            configured_root: Url::parse("https://example.com/").unwrap(),
            root_redirect_rejected: false,
            refused_links: RefusedLinks::default(),
            deny_list: Arc::new(
                RegexSet::new(
                    CrawledUrl::spider_deny_patterns()
                        .iter()
                        .map(|p| p.as_str()),
                )
                .unwrap(),
            ),
            seen: Bloom::new_for_fp_rate(1_000, 0.0001).unwrap(),
            found_links: FoundLinks::new(Arc::new(FollowRules::new(
                Url::parse("https://example.com/").unwrap(),
                whitelist_for("https://example.com/"),
                Arc::new(RegexSet::empty()),
            ))),
            known_pages: Arc::default(),
        }
    }

    /// Storage keeps a page linked in both trailing-slash spellings under the slashed one: the
    /// slashed spelling is admitted after the unslashed one so its row moves there, and an
    /// unslashed spelling after the slashed one is a duplicate of the stored page.
    #[rstest::rstest]
    #[case::unslashed_then_slashed(
        &["https://example.com/p/1", "https://example.com/p/1/", "https://example.com/p/1"],
        &[true, true, false]
    )]
    #[case::slashed_then_unslashed(
        &["https://example.com/p/1/", "https://example.com/p/1", "https://example.com/p/1/"],
        &[true, false, false]
    )]
    #[case::query_keeps_its_page(
        &["https://example.com/p/1/?q=1", "https://example.com/p/1?q=1", "https://example.com/p/1/"],
        &[true, false, true]
    )]
    #[case::root(&["https://example.com/", "https://example.com"], &[true, false])]
    fn should_admit_each_page_once_and_its_slashed_spelling_after_the_unslashed_one(
        #[case] delivered: &[&str],
        #[case] admitted: &[bool],
    ) {
        let mut filter = local_filter();

        let results: Vec<bool> = delivered
            .iter()
            .map(|url| match filter.admit(url) {
                Ok(_) => true,
                Err(LocalRejection::Duplicate) => false,
                Err(other) => panic!("{url}: {other:?}"),
            })
            .collect();

        assert_eq!(results, admitted, "{delivered:?}");
    }

    #[test]
    fn should_only_raise_the_spider_delay_with_the_crawl_delay() {
        assert_eq!(crawl_delay_millis(500, None), 500);
        assert_eq!(
            crawl_delay_millis(500, Some(Duration::from_secs(30))),
            30_000
        );
        assert_eq!(
            crawl_delay_millis(500, Some(Duration::from_millis(100))),
            500
        );
    }

    #[tokio::test]
    async fn should_report_site_refusing_the_crawler_as_root_access_denied() {
        let site = TestSite::spawn(Arc::new(|_: &str, _: usize| {
            SiteResponse::Html("403 Forbidden", String::new())
        }))
        .await;
        let spider = SpiderImpl::with_transport(test_config(), site.transport(&["example.com"]));

        let (_, diagnostics) = run_crawl(&spider, "http://example.com/").await.unwrap();
        let request_log = site.finish().await;

        // A refused robots.txt allows the crawl (RFC 9309), so Spider fetches
        // the root and its own reading of the 403 decides.
        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied),
            "{diagnostics:?}"
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some("library_blocked_status")
        );
        assert_eq!(diagnostics.root_preflight_status, Some(403));
        assert_eq!(diagnostics.http_status, Some(403));
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
            ..
        } = spider
            .crawl_with_body_limit(
                CrawlRequest::from_root("http://example.com/", CrawlPace::default()),
                Some("8388608"),
            )
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
        assert_eq!(
            configured_spider_body_limit(Some("2097152"), 8 * 1024 * 1024).unwrap(),
            2_097_152
        );
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
    fn should_stop_spider_retries_for_truncated_bodies_and_rate_limits_only() {
        let strategy = PageRetryStrategy { max_retries: 2 };
        let outcome = |status_code: u16, content_truncated: bool| AttemptOutcome {
            attempt: 1,
            status_code: spider::reqwest::StatusCode::from_u16(status_code).unwrap(),
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
        assert!(!strategy.on_retry(&outcome(502, true)).should_retry);
        assert!(!strategy.on_retry(&outcome(429, false)).should_retry);
        assert!(strategy.on_retry(&outcome(502, false)).should_retry);
        assert!(strategy.on_retry(&outcome(503, false)).should_retry);
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
            stopped: false,
            status,
            meta,
            initial_html_length: 0,
            initial_should_retry: false,
            initial_anti_bot: AntiBotTech::None,
        }
    }

    #[test]
    fn should_report_a_refused_root_as_root_access_denied() {
        for root_status in [401, 403] {
            let mut diagnostics = CrawlDiagnostics {
                root_preflight_status: Some(root_status),
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
                stopped: false,
                status: CrawlStatus::Blocked,
                meta: WebsiteMetaInfo::None,
                initial_html_length: 1_234,
                initial_should_retry: true,
                initial_anti_bot: AntiBotTech::DataDome,
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
        let page = CrawledPage::new(CrawledUrl::new(
            url::Url::parse("https://example.com/product/1").unwrap(),
        ));

        assert_eq!(page.url.to_string(), "https://example.com/product/1");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage::new(CrawledUrl::new(
            url::Url::parse("https://example.com/about").unwrap(),
        ));

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
            Some("https://other.com/?PHPSESSID=abc"),
            AntiBotTech::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RedirectProblem)
        );
        assert_eq!(
            diagnostics.redirect_url.as_deref(),
            Some("https://other.com/")
        );
        assert_eq!(diagnostics.final_url.as_deref(), Some("https://other.com/"));
    }

    #[test]
    fn should_record_no_redirect_target_for_a_root_that_did_not_redirect() {
        for destination in [None, Some("https://example.com/")] {
            let diagnostics = diagnostics_from_library_page(
                "https://example.com",
                "https://example.com/",
                200,
                destination,
                AntiBotTech::None,
            );

            assert_eq!(diagnostics.redirect_url, None, "{destination:?}");
            assert_eq!(
                diagnostics.final_url.as_deref(),
                Some("https://example.com/"),
                "{destination:?}"
            );
        }
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
