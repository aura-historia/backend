use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
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
    for address in target.addresses {
        builder = builder.resolve(&target.host, address);
    }
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
    canonical_host(left) == canonical_host(right)
}

pub fn url_matches_configured_domain(url: &Url, domain: &str) -> bool {
    url.host_str()
        .is_some_and(|host| canonical_host(host) == canonical_host(domain))
}

pub fn is_publicly_routable_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [first, second, _, _] = address.octets();
            !address.is_unspecified()
                && first != 0
                && !address.is_private()
                && !address.is_loopback()
                && !address.is_link_local()
                && !address.is_broadcast()
                && !address.is_multicast()
                && !(first == 100 && (64..=127).contains(&second))
                && !(first == 192 && second == 0)
                && !(first == 192 && second == 2)
                && !(first == 198 && (second == 18 || second == 19))
                && !(first == 198 && second == 51)
                && !(first == 203 && second == 0)
                && first < 240
        }
        IpAddr::V6(address) => {
            if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
                return false;
            }
            if let Some(address) = address.to_ipv4_mapped() {
                return is_publicly_routable_ip(IpAddr::V4(address));
            }
            let octets = address.octets();
            if octets[..12].iter().all(|octet| *octet == 0) {
                return is_publicly_routable_ip(IpAddr::V4(std::net::Ipv4Addr::new(
                    octets[12], octets[13], octets[14], octets[15],
                )));
            }
            let segments = address.segments();
            (segments[0] & 0xfe00) != 0xfc00
                && (segments[0] & 0xffc0) != 0xfe80
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && !(segments[0] == 0x64 && segments[1] == 0xff9b)
                && !(segments[0] == 0x100 && segments[1] == 0)
        }
    }
}

fn canonical_host(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkErrorKind {
    UnsafeTarget,
    Timeout,
    Connect,
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
/// [`inline_retry_backoff_for`] there so one slow shop cannot block the worker
/// for minutes.
pub fn durable_retry_cooldown_for(kind: NetworkErrorKind) -> Duration {
    match kind {
        NetworkErrorKind::UnsafeTarget => Duration::from_secs(24 * 60 * 60),
        NetworkErrorKind::HttpStatus(429) => Duration::from_secs(10),
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
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(
                !is_publicly_routable_ip(raw_address.parse().unwrap()),
                "{raw_address} must be rejected"
            );
        }
        assert!(is_publicly_routable_ip("1.1.1.1".parse().unwrap()));
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
    fn should_match_only_bare_and_www_host_variants() {
        let bare = Url::parse("https://example.com/products/1").unwrap();
        let www = Url::parse("https://www.example.com/products/1").unwrap();
        let subdomain = Url::parse("https://shop.example.com/products/1").unwrap();

        assert!(is_same_or_www_host(&bare, &www));
        assert!(url_matches_configured_domain(&www, "example.com"));
        assert!(!is_same_or_www_host(&bare, &subdomain));
        assert!(!url_matches_configured_domain(&subdomain, "example.com"));
    }
}
