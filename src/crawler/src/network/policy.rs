use reqwest::header::HeaderValue;
use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, SystemTime},
};
use url::Url;

const ALLOWED_HTTP_PORTS: &[u16] = &[80, 443];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicTargetError {
    #[error("target must use HTTP or HTTPS without userinfo")]
    InvalidUrl,
    #[error("target must use a DNS hostname, not an IP literal")]
    IpLiteral,
    #[error("target port is not permitted")]
    InvalidPort,
    #[error("target DNS resolution timed out")]
    ResolutionTimeout,
    #[error("target DNS resolution failed")]
    Resolution,
    #[error("target does not resolve exclusively to public addresses")]
    UnsafeResolution,
    #[error("redirect target is invalid")]
    InvalidRedirect,
}

/// Stable, scraper-specific health categories persisted on crawler domains.
///
/// This is intentionally separate from [`NetworkErrorKind`]: the latter also
/// contains URL-scoped and security outcomes which must not open a domain
/// circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainFailureKind {
    Timeout,
    Connect,
    DnsTimeout,
    DnsResolution,
    Http408,
    Http429,
    Http503,
    Http504,
    Http5xxBurst,
}

impl DomainFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "TIMEOUT",
            Self::Connect => "CONNECT",
            Self::DnsTimeout => "DNS_TIMEOUT",
            Self::DnsResolution => "DNS_RESOLUTION",
            Self::Http408 => "HTTP_408",
            Self::Http429 => "HTTP_429",
            Self::Http503 => "HTTP_503",
            Self::Http504 => "HTTP_504",
            Self::Http5xxBurst => "HTTP_5XX_BURST",
        }
    }

    pub const fn base_cooldown(self) -> Duration {
        match self {
            Self::Http429 => Duration::from_secs(10 * 60),
            Self::Http503 => Duration::from_secs(15 * 60),
            Self::Http504 => Duration::from_secs(15 * 60),
            Self::Http408
            | Self::Timeout
            | Self::Connect
            | Self::DnsTimeout
            | Self::Http5xxBurst => Duration::from_secs(5 * 60),
            Self::DnsResolution => Duration::from_secs(10 * 60),
        }
    }
}

/// A DNS target resolved immediately before an outbound request.
///
/// Every returned address is public and is pinned into the reqwest client that
/// makes the request, preventing a later DNS rebind from changing the peer.
#[derive(Debug, Clone)]
pub struct PublicTarget {
    pub host: String,
    pub addresses: Vec<SocketAddr>,
}

pub fn validate_public_http_url(url: &Url) -> Result<(&str, u16), PublicTargetError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(PublicTargetError::InvalidUrl);
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(PublicTargetError::InvalidUrl)?;
    if matches!(url.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)))
        || host.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
    {
        return Err(PublicTargetError::IpLiteral);
    }

    let port = url
        .port_or_known_default()
        .filter(|port| ALLOWED_HTTP_PORTS.contains(port))
        .ok_or(PublicTargetError::InvalidPort)?;
    Ok((host, port))
}

pub async fn resolve_public_http_target(
    url: &Url,
    timeout: Duration,
) -> Result<PublicTarget, PublicTargetError> {
    let (host, port) = validate_public_http_url(url)?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Err(PublicTargetError::InvalidUrl);
    }
    let addresses = tokio::time::timeout(timeout, tokio::net::lookup_host((host.as_str(), port)))
        .await
        .map_err(|_| PublicTargetError::ResolutionTimeout)?
        .map_err(|_| PublicTargetError::Resolution)?
        .collect::<Vec<_>>();

    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !is_publicly_routable_ip(address.ip()))
    {
        return Err(PublicTargetError::UnsafeResolution);
    }

    Ok(PublicTarget { host, addresses })
}

pub async fn public_http_client(
    url: &Url,
    timeout: Duration,
    http1_only: bool,
) -> Result<reqwest::Client, PublicTargetError> {
    let target = resolve_public_http_target(url, timeout).await?;
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .no_proxy();
    if http1_only {
        builder = builder.http1_only();
    }
    builder = builder.resolve_to_addrs(&target.host, &target.addresses);
    builder.build().map_err(|_| PublicTargetError::InvalidUrl)
}

pub fn redirect_target(
    current_url: &Url,
    response: &reqwest::Response,
) -> Result<Url, PublicTargetError> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or(PublicTargetError::InvalidRedirect)?
        .to_str()
        .map_err(|_| PublicTargetError::InvalidRedirect)?;
    current_url
        .join(location)
        .map_err(|_| PublicTargetError::InvalidRedirect)
}

pub fn is_same_or_www_host(left: &Url, right: &Url) -> bool {
    let Some(left) = left.host_str() else {
        return false;
    };
    let Some(right) = right.host_str() else {
        return false;
    };
    canonical_crawler_domain(left) == canonical_crawler_domain(right)
}

pub fn url_matches_configured_domain(url: &Url, domain: &str) -> bool {
    url.host_str()
        .is_some_and(|host| canonical_crawler_domain(host) == canonical_crawler_domain(domain))
}

/// Crawler ownership identity: lowercase DNS host, no trailing dot, and at
/// most one leading `www.` removed. This is deliberately crawler-local.
pub fn canonical_crawler_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_owned()
}

/// Compatibility name for the shared outbound HTTP destination policy.
pub fn is_publicly_routable_ip(address: IpAddr) -> bool {
    public_network_policy::is_safe_public_http_destination(address)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkErrorKind {
    UnsafeTarget,
    Timeout,
    Connect,
    DnsTimeout,
    DnsResolution,
    Request,
    HttpStatus(u16),
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAction {
    Retry,
    TerminalRemoved,
    Terminal,
}

pub fn domain_failure_kind(kind: NetworkErrorKind) -> Option<DomainFailureKind> {
    match kind {
        NetworkErrorKind::Timeout => Some(DomainFailureKind::Timeout),
        NetworkErrorKind::Connect => Some(DomainFailureKind::Connect),
        NetworkErrorKind::DnsTimeout => Some(DomainFailureKind::DnsTimeout),
        NetworkErrorKind::DnsResolution => Some(DomainFailureKind::DnsResolution),
        NetworkErrorKind::HttpStatus(408) => Some(DomainFailureKind::Http408),
        NetworkErrorKind::HttpStatus(429) => Some(DomainFailureKind::Http429),
        NetworkErrorKind::HttpStatus(503) => Some(DomainFailureKind::Http503),
        NetworkErrorKind::HttpStatus(504) => Some(DomainFailureKind::Http504),
        _ => None,
    }
}

pub fn network_error_kind_for_public_target_error(error: PublicTargetError) -> NetworkErrorKind {
    match error {
        PublicTargetError::ResolutionTimeout => NetworkErrorKind::DnsTimeout,
        PublicTargetError::Resolution => NetworkErrorKind::DnsResolution,
        PublicTargetError::InvalidUrl
        | PublicTargetError::IpLiteral
        | PublicTargetError::InvalidPort
        | PublicTargetError::UnsafeResolution
        | PublicTargetError::InvalidRedirect => NetworkErrorKind::UnsafeTarget,
    }
}

pub fn parse_retry_after(value: &HeaderValue) -> Option<Duration> {
    parse_retry_after_at(value, SystemTime::now())
}

pub fn parse_retry_after_at(value: &HeaderValue, now: SystemTime) -> Option<Duration> {
    const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
    let value = value.to_str().ok()?.trim();

    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER));
    }

    let date = httpdate::parse_http_date(value).ok()?;
    let delay = date.duration_since(now).ok()?;
    Some(delay.min(MAX_RETRY_AFTER))
}

pub fn domain_cooldown(
    kind: DomainFailureKind,
    failure_streak: u32,
    retry_after: Option<Duration>,
) -> Duration {
    const MAX_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
    let exponent = failure_streak.saturating_sub(1).min(31);
    let multiplier = 1_u32 << exponent;
    let local = kind
        .base_cooldown()
        .saturating_mul(multiplier)
        .min(MAX_COOLDOWN);
    retry_after
        .unwrap_or(Duration::ZERO)
        .max(local)
        .min(MAX_COOLDOWN)
}

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(2),
        }
    }
}

pub fn classify_reqwest_error(err: &reqwest::Error) -> NetworkErrorKind {
    if err.is_timeout() {
        return NetworkErrorKind::Timeout;
    }
    if err.is_connect() {
        return NetworkErrorKind::Connect;
    }
    if err.is_request() {
        return NetworkErrorKind::Request;
    }
    if let Some(status) = err.status() {
        return NetworkErrorKind::HttpStatus(status.as_u16());
    }
    NetworkErrorKind::Unknown
}

pub fn action_for(kind: NetworkErrorKind) -> NetworkAction {
    match kind {
        NetworkErrorKind::HttpStatus(404) | NetworkErrorKind::HttpStatus(410) => {
            NetworkAction::TerminalRemoved
        }
        NetworkErrorKind::HttpStatus(403)
        | NetworkErrorKind::HttpStatus(408)
        | NetworkErrorKind::HttpStatus(425)
        | NetworkErrorKind::HttpStatus(429)
        | NetworkErrorKind::HttpStatus(500)
        | NetworkErrorKind::HttpStatus(502)
        | NetworkErrorKind::HttpStatus(503)
        | NetworkErrorKind::HttpStatus(504)
        | NetworkErrorKind::Timeout
        | NetworkErrorKind::Connect
        | NetworkErrorKind::DnsTimeout
        | NetworkErrorKind::DnsResolution
        | NetworkErrorKind::Request => NetworkAction::Retry,
        NetworkErrorKind::UnsafeTarget
        | NetworkErrorKind::HttpStatus(_)
        | NetworkErrorKind::Unknown => NetworkAction::Terminal,
    }
}

pub fn is_retryable_network_failure(kind: NetworkErrorKind) -> bool {
    action_for(kind) == NetworkAction::Retry
}

pub fn should_adapt_domain_delay(kind: NetworkErrorKind) -> bool {
    matches!(
        kind,
        NetworkErrorKind::HttpStatus(408)
            | NetworkErrorKind::HttpStatus(429)
            | NetworkErrorKind::HttpStatus(503)
            | NetworkErrorKind::HttpStatus(504)
            | NetworkErrorKind::Timeout
            | NetworkErrorKind::Connect
            | NetworkErrorKind::DnsTimeout
            | NetworkErrorKind::DnsResolution
    )
}

pub fn inline_retry_backoff_for(policy: RetryPolicy, attempt: u32) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1));
    let raw_ms = policy
        .base_delay
        .as_millis()
        .saturating_mul(u128::from(factor));
    let capped_ms = raw_ms.min(policy.max_delay.as_millis());
    Duration::from_millis(capped_ms as u64)
}

/// Durable cooldown persisted after all inline fetch attempts fail.
///
/// Do not use this inside a domain worker between fetch attempts; use
/// [`inline_retry_backoff_for`] there so one slow crawl root cannot block the worker
/// for minutes.
pub fn durable_retry_cooldown_for(kind: NetworkErrorKind) -> Duration {
    match kind {
        NetworkErrorKind::UnsafeTarget => Duration::from_secs(24 * 60 * 60),
        NetworkErrorKind::HttpStatus(429) => Duration::from_secs(10 * 60),
        NetworkErrorKind::HttpStatus(503) | NetworkErrorKind::HttpStatus(504) => {
            Duration::from_secs(15 * 60)
        }
        NetworkErrorKind::HttpStatus(403) => Duration::from_secs(2 * 60),
        NetworkErrorKind::HttpStatus(408)
        | NetworkErrorKind::HttpStatus(425)
        | NetworkErrorKind::HttpStatus(500)
        | NetworkErrorKind::HttpStatus(502)
        | NetworkErrorKind::Timeout
        | NetworkErrorKind::Connect
        | NetworkErrorKind::DnsTimeout
        | NetworkErrorKind::DnsResolution
        | NetworkErrorKind::Request
        | NetworkErrorKind::Unknown => Duration::from_secs(5 * 60),
        NetworkErrorKind::HttpStatus(_) => Duration::from_secs(24 * 60 * 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_mark_404_as_terminal_removed() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(404)),
            NetworkAction::TerminalRemoved
        );
    }

    #[test]
    fn should_mark_410_as_terminal_removed() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(410)),
            NetworkAction::TerminalRemoved
        );
    }

    #[test]
    fn should_retry_503() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(503)),
            NetworkAction::Retry
        );
    }

    #[test]
    fn should_retry_403() {
        assert_eq!(
            action_for(NetworkErrorKind::HttpStatus(403)),
            NetworkAction::Retry
        );
    }

    #[test]
    fn should_calculate_durable_cooldown_403_for_two_minutes() {
        assert_eq!(
            durable_retry_cooldown_for(NetworkErrorKind::HttpStatus(403)),
            Duration::from_secs(2 * 60)
        );
    }

    #[test]
    fn should_retry_timeout() {
        assert_eq!(action_for(NetworkErrorKind::Timeout), NetworkAction::Retry);
    }

    #[test]
    fn should_identify_retryable_network_failures() {
        assert!(is_retryable_network_failure(NetworkErrorKind::Timeout));
        assert!(is_retryable_network_failure(NetworkErrorKind::Connect));
        assert!(is_retryable_network_failure(NetworkErrorKind::Request));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            429
        )));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            500
        )));
        assert!(is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            503
        )));
    }

    #[test]
    fn should_adapt_domain_delay_for_domain_health_signals() {
        assert!(should_adapt_domain_delay(NetworkErrorKind::Timeout));
        assert!(should_adapt_domain_delay(NetworkErrorKind::Connect));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(408)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(429)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(503)));
        assert!(should_adapt_domain_delay(NetworkErrorKind::HttpStatus(504)));
    }

    #[test]
    fn should_classify_dns_failures_without_treating_server_errors_as_domain_failures() {
        assert_eq!(
            network_error_kind_for_public_target_error(PublicTargetError::ResolutionTimeout),
            NetworkErrorKind::DnsTimeout
        );
        assert_eq!(
            network_error_kind_for_public_target_error(PublicTargetError::Resolution),
            NetworkErrorKind::DnsResolution
        );
        assert_eq!(domain_failure_kind(NetworkErrorKind::HttpStatus(500)), None);
        assert_eq!(
            domain_failure_kind(NetworkErrorKind::HttpStatus(503)),
            Some(DomainFailureKind::Http503)
        );
    }

    #[test]
    fn should_parse_delta_and_http_date_retry_after_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(
            parse_retry_after_at(&HeaderValue::from_static("120"), now),
            Some(Duration::from_secs(120))
        );

        let future = httpdate::fmt_http_date(SystemTime::UNIX_EPOCH + Duration::from_secs(1_090));
        let future = HeaderValue::from_str(&future).expect("valid HTTP date header");
        assert_eq!(
            parse_retry_after_at(&future, now),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            parse_retry_after_at(&HeaderValue::from_static("not-a-delay"), now),
            None
        );
    }

    #[test]
    fn should_exponentially_backoff_domain_circuits_and_cap_retry_after() {
        assert_eq!(
            domain_cooldown(DomainFailureKind::Http429, 1, None),
            Duration::from_secs(10 * 60)
        );
        assert_eq!(
            domain_cooldown(DomainFailureKind::Http429, 2, None),
            Duration::from_secs(20 * 60)
        );
        assert_eq!(
            domain_cooldown(
                DomainFailureKind::Http429,
                1,
                Some(Duration::from_secs(2 * 60 * 60))
            ),
            Duration::from_secs(2 * 60 * 60)
        );
        assert_eq!(
            domain_cooldown(
                DomainFailureKind::Http429,
                99,
                Some(Duration::from_secs(48 * 60 * 60))
            ),
            Duration::from_secs(24 * 60 * 60)
        );
    }

    #[test]
    fn should_use_fifteen_minute_base_cooldown_for_http_504() {
        assert_eq!(
            DomainFailureKind::Http504.base_cooldown(),
            Duration::from_secs(15 * 60)
        );
    }

    #[test]
    fn should_not_adapt_domain_delay_for_url_scoped_retryable_failures() {
        assert!(!should_adapt_domain_delay(NetworkErrorKind::Request));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            425
        )));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            500
        )));
        assert!(!should_adapt_domain_delay(NetworkErrorKind::HttpStatus(
            502
        )));
    }

    #[test]
    fn should_not_identify_terminal_network_failures_as_retryable() {
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            404
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            410
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::HttpStatus(
            418
        )));
        assert!(!is_retryable_network_failure(NetworkErrorKind::Unknown));
    }

    #[test]
    fn should_calculate_short_inline_retry_backoff_exponentially_with_cap() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(250),
        };

        assert_eq!(
            inline_retry_backoff_for(policy, 1),
            Duration::from_millis(100)
        );
        assert_eq!(
            inline_retry_backoff_for(policy, 2),
            Duration::from_millis(200)
        );
        assert_eq!(
            inline_retry_backoff_for(policy, 3),
            Duration::from_millis(250)
        );
    }

    #[test]
    fn should_cap_default_inline_retry_backoff_at_two_seconds() {
        let policy = RetryPolicy::default();

        assert_eq!(inline_retry_backoff_for(policy, 3), Duration::from_secs(2));
    }

    #[test]
    fn should_reject_ip_literals_userinfo_and_nonstandard_ports() {
        for raw_url in [
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "https://user:password@example.com/",
            "https://example.com:8080/",
        ] {
            let url = Url::parse(raw_url).unwrap();
            assert!(
                validate_public_http_url(&url).is_err(),
                "{raw_url} must be rejected"
            );
        }
    }

    #[test]
    fn should_reject_private_special_use_and_mixed_addresses() {
        for raw_address in [
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "::1",
            "::ffff:1.1.1.1",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "2001:db8::1",
            "4000::1",
            "6000::1",
            "8000::1",
            "a000::1",
            "c000::1",
            "e000::1",
            "64:ff9b:1::1",
            "100:0:0:1::1",
            "3fff::1",
            "5f00::1",
        ] {
            assert!(
                !is_publicly_routable_ip(raw_address.parse().unwrap()),
                "{raw_address} must be rejected"
            );
        }
        assert!(is_publicly_routable_ip("1.1.1.1".parse().unwrap()));
        assert!(is_publicly_routable_ip(
            "2606:4700:4700::1111".parse().unwrap()
        ));
        let public = SocketAddr::from(([1, 1, 1, 1], 443));
        let private = SocketAddr::from(([10, 0, 0, 1], 443));
        assert!(is_publicly_routable_ip(public.ip()));
        assert!(
            ![public, private]
                .iter()
                .all(|address| is_publicly_routable_ip(address.ip()))
        );
    }

    #[test]
    fn should_keep_public_neighbors_of_special_use_prefixes_routable() {
        for raw_address in [
            "192.1.0.1",
            "192.2.0.1",
            "198.50.0.1",
            "198.52.0.1",
            "203.1.0.1",
        ] {
            assert!(
                is_publicly_routable_ip(raw_address.parse().unwrap()),
                "{raw_address} must not be rejected by a broad octet rule"
            );
        }
    }

    #[test]
    fn should_match_only_bare_and_www_host_variants() {
        let bare = Url::parse("https://example.com/products/1").unwrap();
        let www = Url::parse("https://www.example.com/products/1").unwrap();
        let subdomain = Url::parse("https://catalog.example.com/products/1").unwrap();

        assert!(is_same_or_www_host(&bare, &www));
        assert_eq!(canonical_crawler_domain("Example.COM."), "example.com");
        assert_eq!(canonical_crawler_domain("www.example.com"), "example.com");
        assert!(url_matches_configured_domain(&www, "example.com"));
        assert!(!is_same_or_www_host(&bare, &subdomain));
        assert!(!url_matches_configured_domain(&subdomain, "example.com"));
    }
}
