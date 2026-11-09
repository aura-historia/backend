use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

const IMAGE_FETCH_MAX_ATTEMPTS: usize = 5;
const IMAGE_FETCH_MAX_REDIRECTS: usize = 3;
const IMAGE_FETCH_MAX_BYTES: usize = 5 * 1024 * 1024;
const IMAGE_FETCH_INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const IMAGE_FETCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const IMAGE_FETCH_TOTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetches external images after validating every network hop.
///
/// Only HTTP(S) hosts resolving exclusively to public IP addresses are requested. Redirects,
/// response bytes, and request duration are bounded. A failed fetch returns an [`ImageFetchError`];
/// callers degrade to their remaining evidence and log its [`ImageFetchDiagnostics`].
pub struct ImageFetcher {
    request_timeout: Duration,
    total_timeout: Duration,
    max_response_bytes: usize,
    #[cfg(test)]
    unsafe_host_allowlist: Vec<String>,
    #[cfg(test)]
    dns_answers: std::collections::HashMap<String, Vec<SocketAddr>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedImage {
    mime_type: &'static str,
    base64_data: String,
}

impl FetchedImage {
    pub fn mime_type(&self) -> &'static str {
        self.mime_type
    }

    pub fn base64_data(&self) -> &str {
        &self.base64_data
    }
}

impl ImageFetcher {
    pub fn new() -> Self {
        Self {
            request_timeout: IMAGE_FETCH_REQUEST_TIMEOUT,
            total_timeout: IMAGE_FETCH_TOTAL_TIMEOUT,
            max_response_bytes: IMAGE_FETCH_MAX_BYTES,
            #[cfg(test)]
            unsafe_host_allowlist: Vec::new(),
            #[cfg(test)]
            dns_answers: std::collections::HashMap::new(),
        }
    }

    pub async fn fetch(&self, url: &Url) -> Result<FetchedImage, ImageFetchError> {
        let mut attempts = 0;
        let mut last_failure = None;
        let outcome = tokio::time::timeout(self.total_timeout, async {
            loop {
                attempts += 1;
                match self.fetch_once(url).await {
                    Err(failure)
                        if attempts < IMAGE_FETCH_MAX_ATTEMPTS && failure.is_retryable() =>
                    {
                        last_failure = Some(failure);
                        tokio::time::sleep(image_fetch_backoff(attempts)).await;
                    }
                    result => return result,
                }
            }
        })
        .await;
        let failure = match outcome {
            Ok(Ok(image)) => return Ok(image),
            Ok(Err(failure)) => failure,
            // An attempt or backoff that the deadline cuts short has no cause of its own.
            Err(_) => last_failure.unwrap_or(FetchFailure::DeadlineExceeded),
        };
        Err(ImageFetchError { failure, attempts })
    }

    async fn fetch_once(&self, url: &Url) -> Result<FetchedImage, FetchFailure> {
        let mut current_url = url.clone();

        for redirect_count in 0..=IMAGE_FETCH_MAX_REDIRECTS {
            let (host, addresses) = self.resolve_target(&current_url).await?;
            let client = build_image_fetch_client(&host, &addresses, self.request_timeout)?;
            let response = client.get(current_url.clone()).send().await?;

            if response.status().is_redirection() {
                if redirect_count == IMAGE_FETCH_MAX_REDIRECTS {
                    return Err(FetchFailure::RedirectLimitExceeded);
                }
                current_url = redirect_target(&current_url, &response)?;
                continue;
            }

            if !response.status().is_success() {
                return Err(FetchFailure::UnsuccessfulStatus {
                    status: response.status(),
                });
            }
            if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
                let content_type = content_type
                    .to_str()
                    .map_err(|_| FetchFailure::InvalidContentType)?;
                if !content_type_can_be_supported_image(content_type) {
                    return Err(FetchFailure::InvalidContentType);
                }
            }

            let bytes = read_bounded_image_body(response, self.max_response_bytes).await?;
            let mime_type = supported_image_mime_type_from_bytes(&bytes)
                .ok_or(FetchFailure::UnsupportedImage)?;
            return Ok(FetchedImage {
                mime_type,
                base64_data: BASE64.encode(bytes),
            });
        }

        Err(FetchFailure::RedirectLimitExceeded)
    }

    async fn resolve_target(&self, url: &Url) -> Result<(String, Vec<SocketAddr>), FetchFailure> {
        let (host, port) = image_url_target(url)?;
        let host = host.to_owned();
        let addresses =
            tokio::time::timeout(self.request_timeout, self.lookup_addresses(&host, port))
                .await
                .map_err(|_| FetchFailure::ResolutionTimeout)?
                .map_err(FetchFailure::Resolution)?;

        if !self.resolved_addresses_are_safe(&host, &addresses) {
            return Err(FetchFailure::UnsafeTarget);
        }

        Ok((host, addresses))
    }

    async fn lookup_addresses(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        #[cfg(test)]
        if let Some(addresses) = self.dns_answers.get(host) {
            return Ok(addresses.clone());
        }
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }

    #[cfg(not(test))]
    fn resolved_addresses_are_safe(&self, _: &str, addresses: &[SocketAddr]) -> bool {
        resolved_addresses_are_safe(addresses)
    }

    #[cfg(test)]
    fn resolved_addresses_are_safe(&self, host: &str, addresses: &[SocketAddr]) -> bool {
        self.unsafe_host_allowlist
            .iter()
            .any(|allowed_host| allowed_host == host)
            || resolved_addresses_are_safe(addresses)
    }

    #[cfg(test)]
    fn for_test(
        request_timeout: Duration,
        max_response_bytes: usize,
        unsafe_host: impl Into<String>,
    ) -> Self {
        Self {
            request_timeout,
            total_timeout: IMAGE_FETCH_TOTAL_TIMEOUT,
            max_response_bytes,
            unsafe_host_allowlist: vec![unsafe_host.into()],
            dns_answers: std::collections::HashMap::new(),
        }
    }

    /// Answers lookups of `host` with `addresses` instead of the system resolver, so tests can
    /// serve a named host from chosen loopback peers.
    #[cfg(test)]
    fn with_dns_answers(mut self, host: impl Into<String>, addresses: Vec<SocketAddr>) -> Self {
        self.dns_answers.insert(host.into(), addresses);
        self
    }

    /// Bounds all attempts for one image, for interactive callers that would rather omit a
    /// slow image than wait the default budget.
    pub fn with_total_timeout(mut self, total_timeout: Duration) -> Self {
        self.total_timeout = total_timeout;
        self
    }
}

impl Default for ImageFetcher {
    fn default() -> Self {
        Self::new()
    }
}

fn image_url_target(url: &Url) -> Result<(&str, u16), FetchFailure> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(FetchFailure::UnsafeTarget);
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(FetchFailure::UnsafeTarget)?;
    let port = url
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or(FetchFailure::UnsafeTarget)?;
    Ok((host, port))
}

fn build_image_fetch_client(
    host: &str,
    addresses: &[SocketAddr],
    request_timeout: Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    // Reqwest keeps one override per host, and a later one replaces an earlier one. Pinning the
    // whole validated answer set in one call lets the connector fall back from an unreachable peer,
    // such as IPv6 in an IPv4-only network, to a reachable one.
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(request_timeout)
        .connect_timeout(request_timeout)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .resolve_to_addrs(host, addresses)
        .build()
}

fn redirect_target(current_url: &Url, response: &reqwest::Response) -> Result<Url, FetchFailure> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or(FetchFailure::InvalidRedirect)?
        .to_str()
        .map_err(|_| FetchFailure::InvalidRedirect)?;
    current_url
        .join(location)
        .map_err(|_| FetchFailure::InvalidRedirect)
}

async fn read_bounded_image_body(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, FetchFailure> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_response_bytes as u64)
    {
        return Err(FetchFailure::BodyTooLarge);
    }

    let capacity = response
        .content_length()
        .unwrap_or_default()
        .min(max_response_bytes as u64) as usize;
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        let remaining = max_response_bytes.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            return Err(FetchFailure::BodyTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn resolved_addresses_are_safe(addresses: &[SocketAddr]) -> bool {
    !addresses.is_empty()
        && addresses
            .iter()
            .all(|address| is_publicly_routable_ip(address.ip()))
}

fn is_publicly_routable_ip(address: IpAddr) -> bool {
    public_network_policy::is_safe_public_http_destination(address)
}
fn image_fetch_backoff(attempt: usize) -> Duration {
    let multiplier = 1_u32 << attempt.saturating_sub(1);
    IMAGE_FETCH_INITIAL_BACKOFF.saturating_mul(multiplier)
}

fn supported_image_mime_type_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    if bytes.get(4..8) == Some(b"ftyp") {
        let major_brand = bytes.get(8..12)?;
        if matches!(major_brand, b"heic" | b"heix" | b"hevc" | b"hevx") {
            return Some("image/heic");
        }
        if matches!(major_brand, b"mif1" | b"msf1") {
            return Some("image/heif");
        }
    }
    None
}

fn content_type_can_be_supported_image(content_type: &str) -> bool {
    let mime_type = content_type.split(';').next().unwrap_or_default().trim();
    mime_type.eq_ignore_ascii_case("application/octet-stream")
        || mime_type.eq_ignore_ascii_case("binary/octet-stream")
        || mime_type
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
}

/// Why an image could not be fetched: the failure of the last attempt that finished.
///
/// Neither its message nor its sources carry the image URL, which may be signed. Callers recording
/// a failed fetch log [`Self::diagnostics`], never the error chain.
#[derive(Debug, thiserror::Error)]
#[error("image fetch failed after {attempts} attempts")]
pub struct ImageFetchError {
    #[source]
    failure: FetchFailure,
    attempts: usize,
}

impl ImageFetchError {
    /// Bounded, URL-free labels for operational logs.
    pub fn diagnostics(&self) -> ImageFetchDiagnostics {
        ImageFetchDiagnostics {
            category: self.failure.category(),
            http_status: self.failure.http_status(),
            attempts: self.attempts,
        }
    }
}

/// Bounded labels of a failed image fetch, safe for operational logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageFetchDiagnostics {
    /// The last finished attempt's failure, such as `connect` (TCP or TLS), `timeout` or
    /// `unsuccessful_status`; `deadline_exceeded` when no attempt finished in time.
    pub category: &'static str,
    /// The status of an unsuccessful image response.
    pub http_status: Option<u16>,
    /// Attempts started, including one the total deadline cut short.
    pub attempts: usize,
}

#[derive(Debug, thiserror::Error)]
enum FetchFailure {
    #[error("image request failed")]
    Request(#[source] reqwest::Error),
    #[error("image target resolution failed")]
    Resolution(#[source] std::io::Error),
    #[error("image target resolution timed out")]
    ResolutionTimeout,
    #[error("image fetch exceeded its total deadline")]
    DeadlineExceeded,
    #[error("image target is unsafe")]
    UnsafeTarget,
    #[error("image response status {status} is unsuccessful")]
    UnsuccessfulStatus { status: reqwest::StatusCode },
    #[error("image redirect is invalid")]
    InvalidRedirect,
    #[error("image redirect limit exceeded")]
    RedirectLimitExceeded,
    #[error("image content type is unsupported")]
    InvalidContentType,
    #[error("image body exceeds the allowed size")]
    BodyTooLarge,
    #[error("image body is unsupported")]
    UnsupportedImage,
}

impl FetchFailure {
    fn category(&self) -> &'static str {
        match self {
            // The request timeout also bounds connecting and fires first, so a hung connect is a
            // timeout; any other connector failure, TCP or TLS, is a connect failure.
            Self::Request(error) if error.is_timeout() => "timeout",
            Self::Request(error) if error.is_connect() => "connect",
            Self::Request(_) => "request",
            Self::Resolution(_) => "resolution",
            Self::ResolutionTimeout => "resolution_timeout",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::UnsafeTarget => "unsafe_target",
            Self::UnsuccessfulStatus { .. } => "unsuccessful_status",
            Self::InvalidRedirect => "invalid_redirect",
            Self::RedirectLimitExceeded => "redirect_limit_exceeded",
            Self::InvalidContentType => "unsupported_content_type",
            Self::BodyTooLarge => "body_too_large",
            Self::UnsupportedImage => "unsupported_image",
        }
    }

    fn http_status(&self) -> Option<u16> {
        match self {
            Self::UnsuccessfulStatus { status } => Some(status.as_u16()),
            _ => None,
        }
    }

    fn is_retryable(&self) -> bool {
        match self {
            Self::Request(_) | Self::Resolution(_) | Self::ResolutionTimeout => true,
            Self::UnsuccessfulStatus { status } => {
                *status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || *status == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()
            }
            _ => false,
        }
    }
}

impl From<reqwest::Error> for FetchFailure {
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        net::{IpAddr, Ipv4Addr, Ipv6Addr},
        sync::Arc,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
        task::JoinHandle,
    };

    #[test]
    fn should_admit_image_and_binary_content_types_for_body_validation() {
        for content_type in [
            "image/jpeg",
            " IMAGE/PNG ; charset=binary",
            "image/x-custom",
            "application/octet-stream",
            "BINARY/OCTET-STREAM; charset=binary",
        ] {
            assert!(
                content_type_can_be_supported_image(content_type),
                "{content_type}"
            );
        }

        for content_type in [
            "",
            "image",
            "images/png",
            "text/html",
            "application/json",
            "éééé",
        ] {
            assert!(
                !content_type_can_be_supported_image(content_type),
                "{content_type}"
            );
        }
    }

    #[test]
    fn should_detect_supported_image_types_and_retry_with_exponential_backoff() {
        assert_eq!(
            Some("image/jpeg"),
            supported_image_mime_type_from_bytes(&[0xff, 0xd8, 0xff])
        );
        assert_eq!(
            Some("image/png"),
            supported_image_mime_type_from_bytes(b"\x89PNG\r\n\x1a\n")
        );
        assert_eq!(
            Some("image/gif"),
            supported_image_mime_type_from_bytes(b"GIF87a")
        );
        assert_eq!(
            Some("image/webp"),
            supported_image_mime_type_from_bytes(b"RIFFxxxxWEBP")
        );
        assert_eq!(
            Some("image/heic"),
            supported_image_mime_type_from_bytes(b"xxxxftypheic")
        );
        assert_eq!(None, supported_image_mime_type_from_bytes(b"not an image"));
        assert_eq!(Duration::from_millis(100), image_fetch_backoff(1));
        assert_eq!(Duration::from_millis(1_600), image_fetch_backoff(5));
    }

    #[test]
    fn should_reject_non_public_image_targets_and_mixed_dns_answers()
    -> Result<(), Box<dyn std::error::Error>> {
        for address in [
            "0.0.0.0",
            "0.0.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "::",
            "::1",
            "::127.0.0.1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "64:ff9b:1::a00:1",
        ] {
            assert!(
                !is_publicly_routable_ip(address.parse::<IpAddr>()?),
                "{address} must be rejected"
            );
        }
        for address in ["1.1.1.1", "8.8.8.8", "2001:4860:4860::8888"] {
            assert!(is_publicly_routable_ip(address.parse::<IpAddr>()?));
        }

        let public = SocketAddr::from(([8, 8, 8, 8], 443));
        let private = SocketAddr::from(([10, 0, 0, 1], 443));
        assert!(resolved_addresses_are_safe(&[public]));
        assert!(!resolved_addresses_are_safe(&[]));
        assert!(!resolved_addresses_are_safe(&[public, private]));

        let userinfo_url = Url::parse("https://user:password@example.com/image.png")?;
        assert!(matches!(
            image_url_target(&userinfo_url),
            Err(FetchFailure::UnsafeTarget)
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_follow_only_checked_relative_image_redirects()
    -> Result<(), Box<dyn std::error::Error>> {
        let png = b"\x89PNG\r\n\x1a\n";
        let server =
            spawn_mock_image_server(vec![redirect_response("/image"), image_response(png)]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, "127.0.0.1");

        let image = fetcher.fetch_once(&server.url).await?;

        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.base64_data, BASE64.encode(png));
        assert_eq!(
            server.finish().await?,
            vec!["/".to_owned(), "/image".to_owned()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_redirects_to_unsafe_hosts_before_requesting_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let server =
            spawn_mock_image_server(vec![redirect_response("http://localhost/secret")]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, "127.0.0.1");

        assert!(matches!(
            fetcher.fetch_once(&server.url).await,
            Err(FetchFailure::UnsafeTarget)
        ));
        assert_eq!(server.finish().await?, vec!["/".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_image_bodies_that_exceed_the_streaming_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let oversized_png = b"\x89PNG\r\n\x1a\n!";
        let server =
            spawn_mock_image_server(vec![response_without_content_length(oversized_png)]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 8, "127.0.0.1");

        assert!(matches!(
            fetcher.fetch_once(&server.url).await,
            Err(FetchFailure::BodyTooLarge)
        ));
        assert_eq!(server.finish().await?, vec!["/".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_slow_image_responses_at_the_request_timeout()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = delayed(
            image_response(b"\x89PNG\r\n\x1a\n"),
            Duration::from_millis(250),
        );
        let server = spawn_mock_image_server(vec![response]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_millis(25), 64, "127.0.0.1");

        let error = fetcher
            .fetch_once(&server.url)
            .await
            .err()
            .ok_or("slow image was fetched")?;

        assert!(matches!(&error, FetchFailure::Request(source) if source.is_timeout()));
        assert_eq!(error.category(), "timeout");
        assert_eq!(server.finish().await?, vec!["/".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_the_last_attempt_failure_after_retrying()
    -> Result<(), Box<dyn std::error::Error>> {
        let unavailable = http_response("503 Service Unavailable", &[("Content-Length", "0")], &[]);
        let html = http_response(
            "200 OK",
            &[("Content-Type", "text/html"), ("Content-Length", "0")],
            &[],
        );
        let server = spawn_mock_image_server(vec![unavailable, html]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, "127.0.0.1");

        let error = fetcher
            .fetch(&server.url)
            .await
            .err()
            .ok_or("HTML was fetched as an image")?;

        assert_eq!(
            error.diagnostics(),
            ImageFetchDiagnostics {
                category: "unsupported_content_type",
                http_status: None,
                attempts: 2,
            }
        );
        assert_eq!(server.finish().await?, vec!["/".to_owned(), "/".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn should_report_the_last_finished_attempt_when_the_deadline_cuts_a_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let slow = || delayed(image_response(b"\x89PNG\r\n\x1a\n"), Duration::from_secs(2));
        let server = spawn_mock_image_server(vec![slow(), slow()]).await?;
        // Attempts time out at 0.2 s and 0.5 s; the deadline then cuts the second backoff.
        let fetcher = ImageFetcher::for_test(Duration::from_millis(200), 64, "127.0.0.1")
            .with_total_timeout(Duration::from_millis(600));

        let error = fetcher
            .fetch(&server.url)
            .await
            .err()
            .ok_or("slow image was fetched")?;

        assert_eq!(
            error.diagnostics(),
            ImageFetchDiagnostics {
                category: "timeout",
                http_status: None,
                attempts: 2,
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_not_retry_a_permanently_unsuccessful_image_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let not_found = http_response("404 Not Found", &[("Content-Length", "0")], &[]);
        let server = spawn_mock_image_server(vec![not_found]).await?;
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, "127.0.0.1");

        let error = fetcher
            .fetch(&server.url)
            .await
            .err()
            .ok_or("missing image was fetched")?;

        assert_eq!(
            error.diagnostics(),
            ImageFetchDiagnostics {
                category: "unsuccessful_status",
                http_status: Some(404),
                attempts: 1,
            }
        );
        assert_eq!(server.finish().await?, vec!["/".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn should_label_connect_failures_without_exposing_the_image_url()
    -> Result<(), Box<dyn std::error::Error>> {
        const IPV6_ONLY_HOST: &str = "ipv6-only.image.test";
        // Holds the port on IPv4 only, so nothing answers on the IPv6 loopback peer.
        let ipv4_listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = ipv4_listener.local_addr()?.port();
        let unreachable = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
        let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, IPV6_ONLY_HOST)
            .with_dns_answers(IPV6_ONLY_HOST, vec![unreachable]);
        let url = Url::parse(&format!(
            "http://{IPV6_ONLY_HOST}:{port}/image.png?signature=secret-token"
        ))?;

        let error = fetcher
            .fetch_once(&url)
            .await
            .err()
            .ok_or("unreachable peer answered")?;

        assert_eq!(error.category(), "connect");
        let rendered = format!("{error:?} {}", error_chain(&error));
        assert!(!rendered.contains("secret-token"), "{rendered}");
        Ok(())
    }

    #[tokio::test]
    async fn should_fetch_from_a_reachable_pinned_peer_when_another_approved_peer_is_unreachable()
    -> Result<(), Box<dyn std::error::Error>> {
        const DUAL_STACK_HOST: &str = "dual-stack.image.test";
        let png = b"\x89PNG\r\n\x1a\n";
        let server =
            spawn_mock_image_server(vec![image_response(png), image_response(png)]).await?;
        let port = server.url.port().ok_or("mock server URL has no port")?;
        let reachable = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        // The server listens on IPv4 only, like an IPv4-only network reaching a dual-stack host.
        let unreachable = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
        let url = Url::parse(&format!("http://{DUAL_STACK_HOST}:{port}/"))?;

        for peers in [vec![reachable, unreachable], vec![unreachable, reachable]] {
            let fetcher = ImageFetcher::for_test(Duration::from_secs(1), 64, DUAL_STACK_HOST)
                .with_dns_answers(DUAL_STACK_HOST, peers);

            let image = fetcher.fetch_once(&url).await?;

            assert_eq!(image.base64_data, BASE64.encode(png));
        }
        assert_eq!(server.finish().await?, vec!["/".to_owned(), "/".to_owned()]);
        Ok(())
    }

    fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
        std::iter::successors(Some(error), |error| error.source())
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(": ")
    }

    struct MockResponse {
        bytes: Vec<u8>,
        delay: Duration,
    }

    struct MockImageServer {
        url: Url,
        requests: Arc<Mutex<Vec<String>>>,
        task: JoinHandle<io::Result<()>>,
    }

    impl MockImageServer {
        async fn finish(self) -> Result<Vec<String>, io::Error> {
            self.task.await.map_err(io::Error::other)??;
            Ok(self.requests.lock().await.clone())
        }
    }

    async fn spawn_mock_image_server(
        responses: Vec<MockResponse>,
    ) -> Result<MockImageServer, io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_for_task = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await?;
                let mut buffer = [0_u8; 1024];
                let read = socket.read(&mut buffer).await?;
                let path = String::from_utf8_lossy(&buffer[..read])
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                requests_for_task.lock().await.push(path);
                tokio::time::sleep(response.delay).await;
                let _ = socket.write_all(&response.bytes).await;
            }
            Ok(())
        });
        Ok(MockImageServer {
            url: Url::parse(&format!("http://{address}/")).map_err(io::Error::other)?,
            requests,
            task,
        })
    }

    fn image_response(body: &[u8]) -> MockResponse {
        let content_length = body.len().to_string();
        http_response(
            "200 OK",
            &[
                ("Content-Type", "application/octet-stream"),
                ("Content-Length", &content_length),
            ],
            body,
        )
    }

    fn response_without_content_length(body: &[u8]) -> MockResponse {
        http_response(
            "200 OK",
            &[("Content-Type", "application/octet-stream")],
            body,
        )
    }

    fn redirect_response(location: &str) -> MockResponse {
        http_response(
            "302 Found",
            &[("Location", location), ("Content-Length", "0")],
            &[],
        )
    }

    fn delayed(mut response: MockResponse, delay: Duration) -> MockResponse {
        response.delay = delay;
        response
    }

    fn http_response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> MockResponse {
        let mut bytes = format!("HTTP/1.1 {status}\r\nConnection: close\r\n").into_bytes();
        for (name, value) in headers {
            bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(body);
        MockResponse {
            bytes,
            delay: Duration::ZERO,
        }
    }
}
