#[cfg(test)]
use crate::network::policy::crawler_client_builder;
use crate::network::policy::{
    CrawlerClientError, NetworkAction, NetworkErrorKind, PublicTargetError, RequestTimeouts,
    RetryPolicy, action_for, classify_reqwest_error, error_with_sources, inline_retry_backoff_for,
    is_same_or_www_host, network_error_kind_for_public_target_error, parse_retry_after,
    public_http_client, redirect_target,
};
use crate::network::robots::RobotsRefusal;
use crate::scraper::scraper_service::auto_throttle::{
    ScraperAutoThrottle, ScraperAutoThrottleConfig,
};
use robots::RobotsCache;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::debug;
use trailing_slash::TrailingSlashHosts;
use url::Url;

mod robots;
mod trailing_slash;

/// Maximum time for one fetch attempt, including redirects and body read,
/// on a domain whose latency is unknown or low.
const HTML_FETCH_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(15);
/// Upper bound of the attempt timeout once it has grown with the domain's
/// smoothed latency.
const HTML_FETCH_ATTEMPT_TIMEOUT_MAX: Duration = Duration::from_secs(45);
/// Multiple of the domain's smoothed response latency an attempt may take
/// before it is a timeout.
const ADAPTIVE_ATTEMPT_TIMEOUT_LATENCY_FACTOR: u32 = 10;
/// Maximum time for one complete fetch, including retry backoff and all
/// attempts; grows by the same amount as the attempt timeout.
const HTML_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
const HTML_MAX_REDIRECTS: usize = 5;
const HTML_MAX_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// HtmlFetcher trait
// ---------------------------------------------------------------------------

/// Fetches raw HTML from a URL.  The real implementation delegates to
/// [`reqwest::Client`]; tests inject a fake.
#[async_trait::async_trait]
#[mockall::automock]
pub trait HtmlFetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError>;

    /// What the fetcher knows about a domain's responsiveness, for the
    /// diagnostics of a failure that may open the domain circuit. `None`
    /// before the fetcher has a response from the domain.
    fn domain_fetch_stats(&self, _domain: &str) -> Option<DomainFetchStats> {
        None
    }
}

/// Lets one fetcher, with its robots.txt cache and per-domain pacing, serve
/// several owners such as the scraper and the review server.
#[async_trait::async_trait]
impl<T: HtmlFetcher + ?Sized> HtmlFetcher for Arc<T> {
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
        (**self).fetch(url).await
    }

    fn domain_fetch_stats(&self, domain: &str) -> Option<DomainFetchStats> {
        (**self).domain_fetch_stats(domain)
    }
}

/// A domain's smoothed response latency, the responses behind it and the
/// attempt timeout that latency currently earns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainFetchStats {
    pub smoothed_latency: Duration,
    pub latency_samples: u64,
    pub attempt_timeout: Duration,
}

/// The network-time budgets of one fetch on a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FetchBudget {
    attempt: Duration,
    total: Duration,
}

/// The attempt timeout a domain's smoothed latency earns: `base` until the
/// latency's multiple exceeds it, then that multiple up to `max`.
fn adaptive_attempt_timeout(base: Duration, max: Duration, latency: Option<Duration>) -> Duration {
    latency.map_or(base, |latency| {
        latency
            .saturating_mul(ADAPTIVE_ATTEMPT_TIMEOUT_LATENCY_FACTOR)
            .clamp(base, max.max(base))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedHtml {
    pub html: String,
    pub final_url: Url,
    pub status: reqwest::StatusCode,
}

impl FetchedHtml {
    pub fn new(html: String, final_url: Url, status: reqwest::StatusCode) -> Self {
        Self {
            html,
            final_url,
            status,
        }
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
    #[error(
        "network failure: kind={kind:?}, status={status:?}, retry_after={retry_after:?}, details={details}"
    )]
    NetworkWithMetadata {
        kind: NetworkErrorKind,
        status: Option<u16>,
        retry_after: Option<Duration>,
        details: String,
    },
    /// The URL redirected to `final_url`, which robots.txt disallows; the
    /// target was not requested.
    #[error("redirect target is disallowed by robots.txt: final_url={final_url}")]
    RedirectDisallowed { final_url: Url },
}

impl FetchError {
    pub(crate) fn kind(&self) -> NetworkErrorKind {
        match self {
            FetchError::Network { kind, .. } => *kind,
            FetchError::NetworkWithMetadata { kind, .. } => *kind,
            FetchError::RedirectDisallowed { .. } => NetworkErrorKind::RobotsDisallowed,
        }
    }

    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            FetchError::Network { .. } | FetchError::RedirectDisallowed { .. } => None,
            FetchError::NetworkWithMetadata { status, .. } => *status,
        }
    }

    pub(crate) fn retry_after(&self) -> Option<Duration> {
        match self {
            FetchError::Network { .. } | FetchError::RedirectDisallowed { .. } => None,
            FetchError::NetworkWithMetadata { retry_after, .. } => *retry_after,
        }
    }
}

// ---------------------------------------------------------------------------
// ReqwestHtmlFetcher
// ---------------------------------------------------------------------------

pub struct ReqwestHtmlFetcher {
    default_headers: reqwest::header::HeaderMap,
    retry_policy: RetryPolicy,
    attempt_timeout: Duration,
    attempt_timeout_max: Duration,
    total_timeout: Duration,
    auto_throttle: ScraperAutoThrottle,
    robots: RobotsCache,
    trailing_slash: TrailingSlashHosts,
    #[cfg(test)]
    allow_private_targets_for_test: bool,
}

/// What one HTTP request answered.
enum Hop {
    Page(FetchedHtml),
    Redirect(Url),
}

struct FetchAttemptError {
    error: FetchError,
}

enum AttemptError {
    /// May be retried when its kind is retryable.
    Failed(FetchError),
    /// Loading or applying robots.txt failed, for the URL or a redirect
    /// hop; retrying would only fetch robots.txt again.
    Final(FetchError),
    /// The whole fetch ran out of its network-time budget; no retry follows.
    TotalTimeout,
}

/// When an attempt's network time runs out, and whether that is the whole
/// fetch's budget.
#[derive(Clone, Copy)]
struct AttemptLimit {
    at: tokio::time::Instant,
    total: bool,
}

impl AttemptLimit {
    fn new(attempt_deadline: tokio::time::Instant, total_deadline: tokio::time::Instant) -> Self {
        Self {
            at: attempt_deadline.min(total_deadline),
            total: attempt_deadline >= total_deadline,
        }
    }

    /// Runs a robots.txt load or check within the limit. Its failure ends
    /// the fetch: retrying would only fetch robots.txt again.
    async fn robots<T>(
        self,
        step: impl Future<Output = Result<T, FetchAttemptError>>,
    ) -> Result<T, AttemptError> {
        match tokio::time::timeout_at(self.at, step).await {
            Ok(result) => result.map_err(|error| AttemptError::Final(error.error)),
            Err(_) if self.total => Err(AttemptError::TotalTimeout),
            Err(_) => Err(AttemptError::Final(FetchError::Network {
                kind: NetworkErrorKind::Timeout,
                details: "robots.txt fetch exceeded the attempt timeout".to_string(),
            })),
        }
    }

    /// Runs one HTTP request within the limit. Running out of the attempt's
    /// time may be retried; running out of the whole fetch's may not.
    async fn request(
        self,
        step: impl Future<Output = Result<Hop, AttemptError>>,
    ) -> Result<Hop, AttemptError> {
        match tokio::time::timeout_at(self.at, step).await {
            Ok(result) => result,
            Err(_) if self.total => Err(AttemptError::TotalTimeout),
            Err(_) => Err(AttemptError::Failed(FetchError::Network {
                kind: NetworkErrorKind::Timeout,
                details: "HTML fetch attempt exceeded timeout".to_string(),
            })),
        }
    }
}

impl From<FetchAttemptError> for AttemptError {
    fn from(error: FetchAttemptError) -> Self {
        Self::Failed(error.error)
    }
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

        Self {
            default_headers,
            retry_policy,
            attempt_timeout: HTML_FETCH_ATTEMPT_TIMEOUT,
            attempt_timeout_max: HTML_FETCH_ATTEMPT_TIMEOUT_MAX,
            total_timeout: HTML_FETCH_TOTAL_TIMEOUT,
            auto_throttle: ScraperAutoThrottle::new(auto_throttle_config),
            robots: RobotsCache::default(),
            trailing_slash: TrailingSlashHosts::default(),
            #[cfg(test)]
            allow_private_targets_for_test: false,
        }
    }

    /// Returns the origin's robots `Crawl-delay` when robots.txt allows `url`.
    async fn check_robots(&self, url: &Url) -> Result<Option<Duration>, FetchAttemptError> {
        let robots = self.robots_policy(url).await?;
        robots
            .check(url)
            .map_err(|refusal| robots_refusal_error(url, refusal))?;
        Ok(robots.crawl_delay())
    }

    /// Waits for the domain's pacing delay before a request and returns the
    /// time waited: the latency throttle, raised to the robots `Crawl-delay`
    /// spacing when one applies. A caller dropping the fetch while it waits
    /// hands the reserved start back.
    async fn pace(&self, url: &Url, crawl_delay: Option<Duration>) -> Duration {
        let Some(domain) = url.host_str() else {
            return Duration::ZERO;
        };
        let throttle = self.auto_throttle.delay_for(domain);
        let start = self
            .auto_throttle
            .reserve_request_start(domain, throttle, crawl_delay);
        let delay = start.wait();
        if !delay.is_zero() {
            debug!(
                domain,
                delay_ms = delay.as_millis(),
                "Applying scraper domain pacing delay"
            );
            sleep(delay).await;
        }
        start.claim();
        delay
    }

    /// The budgets `url`'s domain earns: the configured ones, with the attempt
    /// grown to a multiple of the domain's smoothed latency within bounds and
    /// the whole fetch grown by the same amount, so a shop that answers slowly
    /// throughout is waited for rather than timed out.
    fn fetch_budget(&self, url: &Url) -> FetchBudget {
        let latency = url
            .host_str()
            .and_then(|domain| self.auto_throttle.latency(domain))
            .map(|latency| latency.smoothed);
        let attempt =
            adaptive_attempt_timeout(self.attempt_timeout, self.attempt_timeout_max, latency);
        FetchBudget {
            attempt,
            total: self.total_timeout + attempt.saturating_sub(self.attempt_timeout),
        }
    }

    /// One fetch attempt: the URL and each redirect hop are admitted by
    /// robots.txt, paced and requested in turn. Loading robots.txt and the
    /// requests count against the attempt and total budgets; every pacing
    /// wait extends both, so neither a long `Crawl-delay` nor the latency
    /// throttle on a slow shop turns into a fetch timeout.
    async fn attempt(
        &self,
        url: &Url,
        deadline: &mut tokio::time::Instant,
    ) -> Result<FetchedHtml, AttemptError> {
        let attempt_timeout = self.fetch_budget(url).attempt;
        let mut attempt_deadline = tokio::time::Instant::now() + attempt_timeout;
        let mut current_url = url.clone();
        for redirect_count in 0..=HTML_MAX_REDIRECTS {
            let limit = AttemptLimit::new(attempt_deadline, *deadline);
            let crawl_delay = if redirect_count == 0 {
                limit.robots(self.check_robots(&current_url)).await?
            } else {
                limit.robots(self.admit_redirect(&current_url)).await?
            };
            let waited = self.pace(&current_url, crawl_delay).await;
            *deadline += waited;
            attempt_deadline += waited;
            let limit = AttemptLimit::new(attempt_deadline, *deadline);
            let next_url = match limit
                .request(self.request_once(&current_url, attempt_timeout))
                .await?
            {
                Hop::Page(page) => return Ok(page),
                Hop::Redirect(next_url) => next_url,
            };
            if redirect_count == HTML_MAX_REDIRECTS {
                break;
            }
            if !is_same_or_www_host(url, &next_url) {
                return Err(AttemptError::Failed(unsafe_fetch_error(
                    "redirect target host does not match configured host",
                )));
            }
            self.trailing_slash
                .observe_redirect(&current_url, &next_url);
            current_url = next_url;
        }

        Err(AttemptError::Failed(unsafe_fetch_error(
            "redirect limit exceeded",
        )))
    }

    /// Checks a redirect hop against robots.txt and returns its origin's
    /// `Crawl-delay`. A target robots.txt disallows is reported with its URL,
    /// so the pipeline can judge where the source URL led.
    async fn admit_redirect(&self, url: &Url) -> Result<Option<Duration>, FetchAttemptError> {
        let robots = self.robots_policy(url).await?;
        match robots.check(url) {
            Ok(()) => Ok(robots.crawl_delay()),
            Err(RobotsRefusal::Disallowed) => Err(FetchAttemptError {
                error: FetchError::RedirectDisallowed {
                    final_url: url.clone(),
                },
            }),
            Err(refusal) => Err(robots_refusal_error(url, refusal)),
        }
    }

    /// Sends one request and reads the page, or the redirect target to follow.
    async fn request_once(
        &self,
        url: &Url,
        attempt_timeout: Duration,
    ) -> Result<Hop, AttemptError> {
        let domain = url.host_str();
        let started = Instant::now();
        let client = self.http_client_for(url, attempt_timeout).await?;
        let response = client
            .get(url.clone())
            .headers(self.default_headers.clone())
            .send()
            .await
            .map_err(reqwest_fetch_error)?;
        let status = response.status();
        if status.is_redirection() {
            self.record_domain_success_latency(domain, started.elapsed());
            let next_url = redirect_target(url, &response).map_err(public_target_fetch_error)?;
            return Ok(Hop::Redirect(next_url));
        }
        if !status.is_success() {
            self.record_domain_failure_latency(domain, started.elapsed());
            return Err(http_status_fetch_error(response).into());
        }
        self.record_domain_success_latency(domain, started.elapsed());
        let final_url = response.url().clone();
        let html = read_bounded_html(response).await?;
        Ok(Hop::Page(FetchedHtml::new(html, final_url, status)))
    }

    /// A client whose request timeout is the domain's attempt timeout while
    /// DNS and connect keep the base timeout: a slow host answers slowly, it
    /// does not connect slowly.
    async fn http_client_for(
        &self,
        url: &Url,
        attempt_timeout: Duration,
    ) -> Result<reqwest::Client, FetchAttemptError> {
        let timeouts = RequestTimeouts {
            connect: self.attempt_timeout,
            request: attempt_timeout,
        };
        #[cfg(test)]
        if self.allow_private_targets_for_test {
            return crawler_client_builder(timeouts)
                .map_err(client_fetch_error)?
                .build()
                .map_err(reqwest_fetch_error);
        }
        public_http_client(url, timeouts)
            .await
            .map_err(client_fetch_error)
    }

    #[cfg(test)]
    fn for_local_http_test(
        retry_policy: RetryPolicy,
        auto_throttle_config: ScraperAutoThrottleConfig,
    ) -> Self {
        Self::for_local_http_test_with_timeouts(
            retry_policy,
            auto_throttle_config,
            HTML_FETCH_ATTEMPT_TIMEOUT,
            HTML_FETCH_TOTAL_TIMEOUT,
        )
    }

    #[cfg(test)]
    fn for_local_http_test_with_timeouts(
        retry_policy: RetryPolicy,
        auto_throttle_config: ScraperAutoThrottleConfig,
        attempt_timeout: Duration,
        total_timeout: Duration,
    ) -> Self {
        let mut fetcher =
            Self::with_retry_policy_and_auto_throttle_config(retry_policy, auto_throttle_config);
        fetcher.attempt_timeout = attempt_timeout;
        // Timing tests set exact budgets; latency growth is tested on its own.
        fetcher.attempt_timeout_max = attempt_timeout;
        fetcher.total_timeout = total_timeout;
        fetcher.allow_private_targets_for_test = true;
        fetcher
    }

    fn record_domain_latency(&self, domain: Option<&str>, latency: Duration) {
        if let Some(domain) = domain {
            self.auto_throttle.record_success_latency(domain, latency);
            debug!(
                domain,
                latency_ms = latency.as_millis(),
                "Recorded scraper fetch latency"
            );
        }
    }

    fn record_domain_success_latency(&self, domain: Option<&str>, latency: Duration) {
        self.record_domain_latency(domain, latency);
    }

    fn record_domain_failure_latency(&self, domain: Option<&str>, latency: Duration) {
        if let Some(domain) = domain {
            self.auto_throttle.record_failure_latency(domain, latency);
            debug!(
                domain,
                latency_ms = latency.as_millis(),
                "Recorded scraper fetch failure latency"
            );
        }
    }
}

fn public_target_fetch_error(error: PublicTargetError) -> FetchAttemptError {
    FetchAttemptError {
        error: FetchError::Network {
            kind: network_error_kind_for_public_target_error(error.clone()),
            details: error.to_string(),
        },
    }
}

fn client_fetch_error(error: CrawlerClientError) -> FetchAttemptError {
    match error {
        CrawlerClientError::Target(error) => public_target_fetch_error(error),
        error @ (CrawlerClientError::TrustStore(_) | CrawlerClientError::Build(_)) => {
            FetchAttemptError {
                error: FetchError::Network {
                    kind: NetworkErrorKind::Unknown,
                    details: error_with_sources(&error),
                },
            }
        }
    }
}

fn total_timeout_error() -> FetchError {
    FetchError::Network {
        kind: NetworkErrorKind::Timeout,
        details: "HTML fetch exceeded total timeout".to_string(),
    }
}

fn robots_refusal_error(url: &Url, refusal: RobotsRefusal) -> FetchAttemptError {
    let (kind, details) = match refusal {
        RobotsRefusal::Disallowed => (NetworkErrorKind::RobotsDisallowed, "robots.txt disallows"),
        RobotsRefusal::Unreachable => (
            NetworkErrorKind::RobotsUnreachable,
            "robots.txt is unreachable, which disallows",
        ),
    };
    FetchAttemptError {
        error: FetchError::Network {
            kind,
            details: format!("{details} {url}"),
        },
    }
}

fn unsafe_fetch_error(details: impl Into<String>) -> FetchError {
    FetchError::Network {
        kind: NetworkErrorKind::UnsafeTarget,
        details: details.into(),
    }
}

fn reqwest_fetch_error(error: reqwest::Error) -> FetchAttemptError {
    FetchAttemptError {
        error: FetchError::Network {
            kind: classify_reqwest_error(&error),
            details: error_with_sources(&error),
        },
    }
}

fn http_status_fetch_error(response: reqwest::Response) -> FetchAttemptError {
    let status = response.status();
    FetchAttemptError {
        error: FetchError::NetworkWithMetadata {
            kind: NetworkErrorKind::HttpStatus(status.as_u16()),
            status: Some(status.as_u16()),
            retry_after: response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(parse_retry_after),
            details: format!("HTTP status {}", status.as_u16()),
        },
    }
}

async fn read_bounded_html(mut response: reqwest::Response) -> Result<String, FetchAttemptError> {
    if response
        .content_length()
        .is_some_and(|length| length > HTML_MAX_BYTES as u64)
    {
        return Err(FetchAttemptError {
            error: unsafe_fetch_error("HTML response exceeds size limit"),
        });
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or_default()
            .min(HTML_MAX_BYTES as u64) as usize,
    );
    while let Some(chunk) = response.chunk().await.map_err(reqwest_fetch_error)? {
        let remaining = HTML_MAX_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            return Err(FetchAttemptError {
                error: unsafe_fetch_error("HTML response exceeds size limit"),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

impl ReqwestHtmlFetcher {
    /// Fetches `url` with inline retries until `deadline`, which pacing waits
    /// extend.
    async fn fetch_within(
        &self,
        url: &Url,
        deadline: &mut tokio::time::Instant,
    ) -> Result<FetchedHtml, FetchError> {
        for attempt in 1..=self.retry_policy.max_attempts {
            let error = match self.attempt(url, deadline).await {
                Ok(html) => return Ok(html),
                Err(AttemptError::Failed(error)) => error,
                Err(AttemptError::Final(error)) => return Err(error),
                Err(AttemptError::TotalTimeout) => return Err(total_timeout_error()),
            };
            if action_for(error.kind()) != NetworkAction::Retry
                || attempt >= self.retry_policy.max_attempts
            {
                return Err(error);
            }
            let backoff = inline_retry_backoff_for(self.retry_policy, attempt);
            let retry_after = error.retry_after();
            if retry_after.is_some_and(|delay| delay > self.retry_policy.max_delay) {
                return Err(error);
            }
            let backoff = retry_after.map_or(backoff, |delay| delay.max(backoff));
            if tokio::time::Instant::now() + backoff >= *deadline {
                return Err(total_timeout_error());
            }
            if !backoff.is_zero() {
                sleep(backoff).await;
            }
        }

        Err(FetchError::Network {
            kind: NetworkErrorKind::Unknown,
            details: "retry loop terminated unexpectedly".to_string(),
        })
    }
}

/// Whether a failure of the trailing-slash form says nothing about the stored
/// form: the slashed page does not exist, or robots.txt refuses that form. The
/// stored form is then fetched and judged as it would be without the guess.
fn needs_stored_form(error: &FetchError) -> bool {
    matches!(error.status(), Some(404 | 410))
        || matches!(
            error,
            FetchError::Network {
                kind: NetworkErrorKind::RobotsDisallowed,
                ..
            }
        )
}

#[async_trait::async_trait]
impl HtmlFetcher for ReqwestHtmlFetcher {
    /// Pacing waits extend the timeout budgets: they bound network work, so a
    /// long robots `Crawl-delay` never turns into a fetch timeout. On a host
    /// that redirects to trailing-slash paths the slashed form is requested
    /// first, within the same budget.
    async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
        let mut deadline = tokio::time::Instant::now() + self.fetch_budget(url).total;
        if let Some(slashed) = self.trailing_slash.preferred_form(url) {
            match self.fetch_within(&slashed, &mut deadline).await {
                Err(error) if needs_stored_form(&error) => {
                    debug!(
                        url = %slashed,
                        error = %error,
                        "Trailing-slash form failed; fetching the stored form"
                    );
                }
                result => return result,
            }
        }
        self.fetch_within(url, &mut deadline).await
    }

    fn domain_fetch_stats(&self, domain: &str) -> Option<DomainFetchStats> {
        let latency = self.auto_throttle.latency(domain)?;
        Some(DomainFetchStats {
            smoothed_latency: latency.smoothed,
            latency_samples: latency.samples,
            attempt_timeout: adaptive_attempt_timeout(
                self.attempt_timeout,
                self.attempt_timeout_max,
                Some(latency.smoothed),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FetchError, HtmlFetcher, ReqwestHtmlFetcher, client_fetch_error};
    use crate::network::policy::{CrawlerClientError, NetworkErrorKind, RetryPolicy};
    use crate::scraper::scraper_service::ScraperAutoThrottleConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use url::Url;

    const OK_PAGE: &str =
        "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>";

    fn robots_txt(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn robots_status(status_line: &str) -> String {
        format!("HTTP/1.1 {status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// A local shop that answers every `/robots.txt` request with `robots`
    /// and every other request with the next of `pages`, after `page_delay`.
    struct TestShop {
        url: Url,
        page_requests: Arc<Mutex<Vec<(Instant, String)>>>,
        robots_requests: Arc<AtomicUsize>,
    }

    impl TestShop {
        fn page_requests(&self) -> Vec<Instant> {
            self.page_requests
                .lock()
                .unwrap()
                .iter()
                .map(|(at, _)| *at)
                .collect()
        }

        fn page_paths(&self) -> Vec<String> {
            self.page_requests
                .lock()
                .unwrap()
                .iter()
                .map(|(_, path)| path.clone())
                .collect()
        }

        fn url_with_path(&self, path: &str) -> Url {
            self.url.join(path).unwrap()
        }
    }

    async fn spawn_shop(
        robots: String,
        pages: Vec<&'static str>,
        page_delay: Duration,
    ) -> TestShop {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let page_requests = Arc::new(Mutex::new(Vec::new()));
        let robots_requests = Arc::new(AtomicUsize::new(0));
        let shop = TestShop {
            url: Url::parse(&format!("http://{addr}/product")).unwrap(),
            page_requests: Arc::clone(&page_requests),
            robots_requests: Arc::clone(&robots_requests),
        };

        tokio::spawn(async move {
            let mut pages = pages.into_iter();
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 1024];
                let read = socket.read(&mut buffer).await.unwrap_or_default();
                if buffer[..read].starts_with(b"GET /robots.txt ") {
                    robots_requests.fetch_add(1, Ordering::SeqCst);
                    let _ = socket.write_all(robots.as_bytes()).await;
                    continue;
                }
                let path = String::from_utf8_lossy(&buffer[..read])
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                page_requests.lock().unwrap().push((Instant::now(), path));
                let Some(page) = pages.next() else {
                    return;
                };
                tokio::time::sleep(page_delay).await;
                let _ = socket.write_all(page.as_bytes()).await;
            }
        });

        shop
    }

    async fn spawn_shop_without_robots(pages: Vec<&'static str>) -> Url {
        spawn_shop(robots_status("404 Not Found"), pages, Duration::ZERO)
            .await
            .url
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

    #[tokio::test]
    async fn reqwest_fetcher_rejects_an_ip_literal_target_before_connecting() {
        let fetcher = ReqwestHtmlFetcher::with_retry_policy_and_auto_throttle_config(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
        );

        let error = fetcher
            .fetch(&Url::parse("http://127.0.0.1/product").unwrap())
            .await
            .expect_err("an IP literal is never a public crawl target");

        assert!(matches!(
            error,
            FetchError::Network {
                kind: NetworkErrorKind::UnsafeTarget,
                ..
            }
        ));
    }

    /// A client that cannot be built says nothing about the target: it must
    /// neither mark the URL unsafe nor hide the cause.
    #[test]
    fn client_fetch_error_should_keep_the_cause_of_an_unavailable_trust_store() {
        let error = client_fetch_error(CrawlerClientError::TrustStore(rustls::Error::General(
            "no CA certificates were loaded".to_owned(),
        )))
        .error;

        let FetchError::Network { kind, details } = error else {
            panic!("expected a network fetch error, got {error:?}");
        };
        assert_eq!(kind, NetworkErrorKind::Unknown);
        assert!(
            details.contains("platform TLS trust store is unavailable")
                && details.contains("no CA certificates were loaded"),
            "{details}"
        );
    }

    #[tokio::test]
    async fn reqwest_fetcher_stops_inline_retries_for_long_retry_after() {
        let url = spawn_shop_without_robots(vec![
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 300\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;

        let fetcher = ReqwestHtmlFetcher::for_local_http_test(
            zero_delay_retry_policy(2),
            throttle_config(Duration::ZERO),
        );

        let error = tokio::time::timeout(Duration::from_millis(100), fetcher.fetch(&url))
            .await
            .expect("long Retry-After must not block the worker")
            .expect_err("long Retry-After must stop remaining inline attempts");

        assert!(matches!(
            error,
            FetchError::NetworkWithMetadata {
                kind: NetworkErrorKind::HttpStatus(429),
                retry_after: Some(delay),
                ..
            } if delay == Duration::from_secs(300)
        ));
    }

    #[tokio::test]
    async fn reqwest_fetcher_uses_short_inline_backoff_for_retryable_status() {
        let url = spawn_shop_without_robots(vec![
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test(
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

    #[test]
    fn adaptive_attempt_timeout_should_grow_with_latency_within_bounds() {
        let base = Duration::from_secs(15);
        let max = Duration::from_secs(45);

        assert_eq!(super::adaptive_attempt_timeout(base, max, None), base);
        assert_eq!(
            super::adaptive_attempt_timeout(base, max, Some(Duration::from_millis(300))),
            base,
            "a fast shop keeps the base timeout"
        );
        assert_eq!(
            super::adaptive_attempt_timeout(base, max, Some(Duration::from_millis(2_200))),
            Duration::from_secs(22)
        );
        assert_eq!(
            super::adaptive_attempt_timeout(base, max, Some(Duration::from_secs(9))),
            max
        );
    }

    /// A shop that answers slowly throughout earns a longer attempt timeout
    /// from its smoothed latency instead of timing out at the base.
    #[tokio::test]
    async fn should_extend_the_attempt_timeout_for_a_slow_domain() {
        let fast = spawn_shop(
            robots_status("404 Not Found"),
            vec![OK_PAGE],
            Duration::from_millis(30),
        )
        .await;
        let slow = spawn_shop(
            robots_status("404 Not Found"),
            vec![OK_PAGE],
            Duration::from_millis(150),
        )
        .await;
        let mut fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(200),
        );
        fetcher.attempt_timeout_max = Duration::from_secs(2);

        // Both shops share the loopback domain, so the first response sets
        // the latency the second fetch is budgeted by.
        fetcher.fetch(&fast.url).await.unwrap();
        let stats = fetcher
            .domain_fetch_stats(fast.url.host_str().unwrap())
            .expect("one response gives the domain a latency");
        assert_eq!(stats.latency_samples, 1);
        assert!(
            stats.attempt_timeout >= Duration::from_millis(300),
            "{stats:?}"
        );
        assert!(stats.attempt_timeout <= Duration::from_secs(2), "{stats:?}");

        let fetched = fetcher.fetch(&slow.url).await.unwrap();
        assert_eq!(fetched.html, "<html>ok</html>");
    }

    #[tokio::test]
    async fn reqwest_fetcher_reports_attempt_timeout_before_total_timeout() {
        let url = spawn_shop(
            robots_status("404 Not Found"),
            vec![OK_PAGE],
            Duration::from_millis(100),
        )
        .await
        .url;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
            Duration::from_millis(20),
            Duration::from_millis(200),
        );

        let started = Instant::now();
        let error = fetcher.fetch(&url).await.unwrap_err();

        assert!(matches!(
            error,
            crate::scraper::scraper_service::FetchError::Network {
                kind: crate::network::policy::NetworkErrorKind::Timeout,
                ..
            }
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn reqwest_fetcher_stops_retries_when_total_timeout_expires() {
        let url = spawn_shop_without_robots(vec![
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ])
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            RetryPolicy {
                max_attempts: 3,
                base_delay: Duration::from_millis(40),
                max_delay: Duration::from_millis(40),
            },
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(50),
        );

        let error = fetcher.fetch(&url).await.unwrap_err();

        assert!(matches!(
            error,
            crate::scraper::scraper_service::FetchError::Network {
                kind: crate::network::policy::NetworkErrorKind::Timeout,
                details,
            } if details == "HTML fetch exceeded total timeout"
        ));
    }

    #[tokio::test]
    async fn reqwest_fetcher_applies_throttle_floor_before_fetch() {
        let url = spawn_shop_without_robots(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n<html>ok</html>",
        ])
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test(
            zero_delay_retry_policy(1),
            throttle_config(Duration::from_millis(40)),
        );

        let started = Instant::now();
        let fetched = fetcher.fetch(&url).await.unwrap();

        assert_eq!(fetched.html, "<html>ok</html>");
        assert_eq!(fetched.final_url, url);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    fn unthrottled_fetcher() -> ReqwestHtmlFetcher {
        ReqwestHtmlFetcher::for_local_http_test(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
        )
    }

    #[tokio::test]
    async fn should_skip_a_url_robots_txt_disallows_without_requesting_it() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nDisallow: /product\n"),
            vec![OK_PAGE],
            Duration::ZERO,
        )
        .await;

        let error = unthrottled_fetcher().fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::RobotsDisallowed);
        assert!(shop.page_requests().is_empty());
    }

    #[tokio::test]
    async fn should_fetch_a_url_robots_txt_allows() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nDisallow: /cart\n\nUser-agent: AhrefsBot\nDisallow: /\n"),
            vec![OK_PAGE],
            Duration::ZERO,
        )
        .await;

        let fetched = unthrottled_fetcher().fetch(&shop.url).await.unwrap();

        assert_eq!(fetched.html, "<html>ok</html>");
        assert_eq!(shop.page_requests().len(), 1);
    }

    #[tokio::test]
    async fn should_space_requests_by_the_crawl_delay_without_timing_out() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nCrawl-delay: 0.3\n"),
            vec![OK_PAGE, OK_PAGE],
            Duration::ZERO,
        )
        .await;
        // The crawl-delay exceeds both timeout budgets: pacing must not count.
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(200),
        );

        fetcher.fetch(&shop.url).await.unwrap();
        fetcher.fetch(&shop.url).await.unwrap();

        let requests = shop.page_requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1] - requests[0] >= Duration::from_millis(300));
        assert_eq!(shop.robots_requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn should_allow_every_url_when_robots_txt_answers_4xx() {
        for status in ["404 Not Found", "403 Forbidden"] {
            let shop = spawn_shop(robots_status(status), vec![OK_PAGE], Duration::ZERO).await;

            let fetched = unthrottled_fetcher().fetch(&shop.url).await;

            assert!(
                fetched.is_ok(),
                "robots.txt {status} must allow: {fetched:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_disallow_the_origin_while_robots_txt_answers_5xx_or_429() {
        for status in ["503 Service Unavailable", "429 Too Many Requests"] {
            let shop = spawn_shop(robots_status(status), vec![OK_PAGE], Duration::ZERO).await;
            let fetcher = unthrottled_fetcher();

            let first = fetcher.fetch(&shop.url).await.unwrap_err();
            let second = fetcher.fetch(&shop.url).await.unwrap_err();

            assert_eq!(
                first.kind(),
                NetworkErrorKind::RobotsUnreachable,
                "{status}"
            );
            assert_eq!(
                second.kind(),
                NetworkErrorKind::RobotsUnreachable,
                "{status}"
            );
            assert!(shop.page_requests().is_empty());
            assert_eq!(shop.robots_requests.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn should_follow_a_robots_txt_redirect_to_another_host() {
        let robots_host = spawn_shop(
            robots_txt("User-agent: *\nDisallow: /\n"),
            vec![],
            Duration::ZERO,
        )
        .await;
        let redirect = format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost:{}/robots.txt\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            robots_host.url.port().unwrap()
        );
        let shop = spawn_shop(redirect, vec![OK_PAGE], Duration::ZERO).await;

        let error = unthrottled_fetcher().fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::RobotsDisallowed);
        assert_eq!(robots_host.robots_requests.load(Ordering::SeqCst), 1);
        assert!(shop.page_requests().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn should_fetch_robots_txt_again_after_the_unreachable_retry_period() {
        let shop = spawn_shop(
            robots_status("500 Internal Server Error"),
            vec![],
            Duration::ZERO,
        )
        .await;
        let fetcher = unthrottled_fetcher();

        let _ = fetcher.fetch(&shop.url).await;
        tokio::time::advance(Duration::from_secs(15 * 60 + 1)).await;
        let _ = fetcher.fetch(&shop.url).await;

        assert_eq!(shop.robots_requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_disallow_the_origin_when_a_robots_txt_redirect_target_host_fails() {
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        let redirect = format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost:{closed_port}/robots.txt\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let shop = spawn_shop(redirect, vec![OK_PAGE], Duration::ZERO).await;

        let error = unthrottled_fetcher().fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::RobotsUnreachable);
        assert!(shop.page_requests().is_empty());
    }

    #[tokio::test]
    async fn should_disallow_the_origin_when_a_redirected_robots_txt_body_breaks_off() {
        let robots_host = spawn_shop(
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nUser-agent"
                .to_owned(),
            vec![],
            Duration::ZERO,
        )
        .await;
        let redirect = format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: http://localhost:{}/robots.txt\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            robots_host.url.port().unwrap()
        );
        let shop = spawn_shop(redirect, vec![OK_PAGE], Duration::ZERO).await;

        let error = unthrottled_fetcher().fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::RobotsUnreachable);
        assert!(shop.page_requests().is_empty());
    }

    #[tokio::test]
    async fn should_not_retry_a_redirect_whose_robots_txt_fails_to_load() {
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        let redirect: &'static str = Box::leak(
            format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: http://127.0.0.1:{closed_port}/product\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_boxed_str(),
        );
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![redirect, redirect],
            Duration::ZERO,
        )
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test(
            zero_delay_retry_policy(3),
            throttle_config(Duration::ZERO),
        );

        let error = fetcher.fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::Connect);
        assert_eq!(shop.page_requests().len(), 1);
    }

    #[tokio::test]
    async fn should_not_retry_a_redirect_whose_robots_txt_stalls() {
        let stalled = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stalled_port = stalled.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = stalled.accept().await {
                held.push(socket);
            }
        });
        let redirect: &'static str = Box::leak(
            format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: http://127.0.0.1:{stalled_port}/product\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_boxed_str(),
        );
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![redirect, redirect],
            Duration::ZERO,
        )
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(3),
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(500),
        );

        let error = fetcher.fetch(&shop.url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::Timeout);
        assert_eq!(shop.page_requests().len(), 1);
    }

    fn redirect_to(location: &str) -> &'static str {
        Box::leak(
            format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_boxed_str(),
        )
    }

    const NOT_FOUND_PAGE: &str =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    #[tokio::test]
    async fn should_follow_a_redirect_whose_hop_pacing_outlasts_the_attempt_timeout() {
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![redirect_to("/moved"), OK_PAGE],
            Duration::ZERO,
        )
        .await;
        // The latency throttle spaces requests further apart than an attempt
        // may take: pacing must not count, for the redirect hop either.
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(1),
            throttle_config(Duration::from_millis(200)),
            Duration::from_millis(100),
            Duration::from_millis(300),
        );

        let fetched = fetcher.fetch(&shop.url).await.unwrap();

        assert_eq!(fetched.final_url.path(), "/moved");
        assert_eq!(shop.page_paths(), ["/product", "/moved"]);
    }

    #[tokio::test]
    async fn should_space_a_redirect_hop_by_the_crawl_delay_without_timing_out() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nCrawl-delay: 0.3\n"),
            vec![redirect_to("/moved"), OK_PAGE],
            Duration::ZERO,
        )
        .await;
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(1),
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(200),
        );

        fetcher.fetch(&shop.url).await.unwrap();

        let requests = shop.page_requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1] - requests[0] >= Duration::from_millis(300));
    }

    #[tokio::test]
    async fn should_request_the_trailing_slash_form_on_a_host_that_redirects_to_it() {
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![redirect_to("/product/"), OK_PAGE, OK_PAGE],
            Duration::ZERO,
        )
        .await;
        let fetcher = unthrottled_fetcher();

        fetcher.fetch(&shop.url).await.unwrap();
        let fetched = fetcher.fetch(&shop.url_with_path("/other")).await.unwrap();

        assert_eq!(fetched.final_url.path(), "/other/");
        assert_eq!(shop.page_paths(), ["/product", "/product/", "/other/"]);
    }

    #[tokio::test]
    async fn should_fetch_the_stored_form_when_the_trailing_slash_form_is_missing() {
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![redirect_to("/product/"), OK_PAGE, NOT_FOUND_PAGE, OK_PAGE],
            Duration::ZERO,
        )
        .await;
        let fetcher = unthrottled_fetcher();

        fetcher.fetch(&shop.url).await.unwrap();
        let fetched = fetcher.fetch(&shop.url_with_path("/other")).await.unwrap();

        assert_eq!(fetched.final_url.path(), "/other");
        assert_eq!(
            shop.page_paths(),
            ["/product", "/product/", "/other/", "/other"]
        );
    }

    #[tokio::test]
    async fn should_request_stored_forms_again_once_the_host_redirects_a_slashed_path_away() {
        let shop = spawn_shop(
            robots_status("404 Not Found"),
            vec![
                redirect_to("/product/"),
                OK_PAGE,
                redirect_to("/other"),
                OK_PAGE,
                OK_PAGE,
            ],
            Duration::ZERO,
        )
        .await;
        let fetcher = unthrottled_fetcher();

        fetcher.fetch(&shop.url).await.unwrap();
        fetcher.fetch(&shop.url_with_path("/other")).await.unwrap();
        fetcher.fetch(&shop.url_with_path("/third")).await.unwrap();

        assert_eq!(
            shop.page_paths(),
            ["/product", "/product/", "/other/", "/other", "/third"]
        );
    }

    #[tokio::test]
    async fn should_bound_a_stalled_robots_txt_by_the_attempt_timeout_without_retrying() {
        // Accepts connections and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let accepted = Arc::clone(&connections);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                accepted.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        let fetcher = ReqwestHtmlFetcher::for_local_http_test_with_timeouts(
            zero_delay_retry_policy(3),
            throttle_config(Duration::ZERO),
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        let url = Url::parse(&format!("http://{addr}/product")).unwrap();

        let started = Instant::now();
        let error = fetcher.fetch(&url).await.unwrap_err();

        assert_eq!(error.kind(), NetworkErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_millis(400));
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn should_report_a_redirect_target_robots_txt_disallows_with_its_url() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nDisallow: /sold/\n"),
            vec!["HTTP/1.1 301 Moved Permanently\r\nLocation: /sold/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"],
            Duration::ZERO,
        )
        .await;

        let error = unthrottled_fetcher().fetch(&shop.url).await.unwrap_err();

        assert!(
            matches!(
                &error,
                FetchError::RedirectDisallowed { final_url } if final_url.path() == "/sold/"
            ),
            "{error:?}"
        );
        assert_eq!(shop.page_requests().len(), 1);
    }

    #[tokio::test]
    async fn should_fetch_robots_txt_once_for_concurrent_requests_to_one_origin() {
        let shop = spawn_shop(
            robots_txt("User-agent: *\nDisallow: /cart\n"),
            vec![OK_PAGE, OK_PAGE],
            Duration::ZERO,
        )
        .await;
        let fetcher = unthrottled_fetcher();

        let (first, second) = tokio::join!(fetcher.fetch(&shop.url), fetcher.fetch(&shop.url));

        assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
        assert_eq!(shop.robots_requests.load(Ordering::SeqCst), 1);
    }
}
