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
/// response bytes, and request duration are bounded. Fetch failures intentionally yield no image.
pub struct ImageFetcher {
    request_timeout: Duration,
    max_response_bytes: usize,
    #[cfg(test)]
    unsafe_host_allowlist: Vec<String>,
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
            max_response_bytes: IMAGE_FETCH_MAX_BYTES,
            #[cfg(test)]
            unsafe_host_allowlist: Vec::new(),
        }
    }

    pub async fn fetch(&self, url: &Url) -> Option<FetchedImage> {
        tokio::time::timeout(IMAGE_FETCH_TOTAL_TIMEOUT, self.fetch_with_retries(url))
            .await
            .ok()
            .flatten()
    }

    async fn fetch_with_retries(&self, url: &Url) -> Option<FetchedImage> {
        for attempt in 1..=IMAGE_FETCH_MAX_ATTEMPTS {
            match self.fetch_once(url).await {
                Ok(image) => return Some(image),
                Err(error) if attempt == IMAGE_FETCH_MAX_ATTEMPTS || !error.is_retryable() => {
                    return None;
                }
                Err(_) => tokio::time::sleep(image_fetch_backoff(attempt)).await,
            }
        }
        None
    }

    async fn fetch_once(&self, url: &Url) -> Result<FetchedImage, ImageFetchError> {
        let mut current_url = url.clone();

        for redirect_count in 0..=IMAGE_FETCH_MAX_REDIRECTS {
            let (host, addresses) = self.resolve_target(&current_url).await?;
            let client = build_image_fetch_client(&host, &addresses, self.request_timeout)?;
            let response = client.get(current_url.clone()).send().await?;

            if response.status().is_redirection() {
                if redirect_count == IMAGE_FETCH_MAX_REDIRECTS {
                    return Err(ImageFetchError::RedirectLimitExceeded);
                }
                current_url = redirect_target(&current_url, &response)?;
                continue;
            }

            if !response.status().is_success() {
                return Err(ImageFetchError::Response);
            }
            if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
                let content_type = content_type
                    .to_str()
                    .map_err(|_| ImageFetchError::InvalidContentType)?;
                if !content_type_can_be_supported_image(content_type) {
                    return Err(ImageFetchError::InvalidContentType);
                }
            }

            let bytes = read_bounded_image_body(response, self.max_response_bytes).await?;
            let mime_type = supported_image_mime_type_from_bytes(&bytes)
                .ok_or(ImageFetchError::UnsupportedImage)?;
            return Ok(FetchedImage {
                mime_type,
                base64_data: BASE64.encode(bytes),
            });
        }

        Err(ImageFetchError::RedirectLimitExceeded)
    }

    async fn resolve_target(
        &self,
        url: &Url,
    ) -> Result<(String, Vec<SocketAddr>), ImageFetchError> {
        let (host, port) = image_url_target(url)?;
        let host = host.to_owned();
        let addresses = tokio::time::timeout(
            self.request_timeout,
            tokio::net::lookup_host((host.as_str(), port)),
        )
        .await
        .map_err(|_| ImageFetchError::Timeout)?
        .map_err(ImageFetchError::Resolution)?
        .collect::<Vec<_>>();

        if !self.resolved_addresses_are_safe(&host, &addresses) {
            return Err(ImageFetchError::UnsafeTarget);
        }

        Ok((host, addresses))
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
            max_response_bytes,
            unsafe_host_allowlist: vec![unsafe_host.into()],
        }
    }
}

impl Default for ImageFetcher {
    fn default() -> Self {
        Self::new()
    }
}

fn image_url_target(url: &Url) -> Result<(&str, u16), ImageFetchError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ImageFetchError::UnsafeTarget);
    }

    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or(ImageFetchError::UnsafeTarget)?;
    let port = url
        .port_or_known_default()
        .filter(|port| *port != 0)
        .ok_or(ImageFetchError::UnsafeTarget)?;
    Ok((host, port))
}

fn build_image_fetch_client(
    host: &str,
    addresses: &[SocketAddr],
    request_timeout: Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(request_timeout)
        .connect_timeout(request_timeout)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd();
    for address in addresses {
        builder = builder.resolve(host, *address);
    }
    builder.build()
}

fn redirect_target(
    current_url: &Url,
    response: &reqwest::Response,
) -> Result<Url, ImageFetchError> {
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .ok_or(ImageFetchError::InvalidRedirect)?
        .to_str()
        .map_err(|_| ImageFetchError::InvalidRedirect)?;
    current_url
        .join(location)
        .map_err(|_| ImageFetchError::InvalidRedirect)
}

async fn read_bounded_image_body(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, ImageFetchError> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_response_bytes as u64)
    {
        return Err(ImageFetchError::BodyTooLarge);
    }

    let capacity = response
        .content_length()
        .unwrap_or_default()
        .min(max_response_bytes as u64) as usize;
    let mut bytes = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        let remaining = max_response_bytes.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            return Err(ImageFetchError::BodyTooLarge);
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
            !address.is_unspecified()
                && !address.is_loopback()
                && !address.is_multicast()
                && (segments[0] & 0xfe00) != 0xfc00
                && (segments[0] & 0xffc0) != 0xfe80
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
                && segments[0] != 0x2002
                && !(segments[0] == 0x64 && segments[1] == 0xff9b)
                && !(segments[0] == 0x100 && segments[1] == 0)
        }
    }
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
    supported_image_mime_type_from_content_type(mime_type).is_some()
        || matches!(
            mime_type.to_ascii_lowercase().as_str(),
            "application/octet-stream" | "binary/octet-stream"
        )
        || mime_type.to_ascii_lowercase().starts_with("image/")
}

fn supported_image_mime_type_from_content_type(content_type: &str) -> Option<&'static str> {
    if matches!(content_type, "image/jpeg" | "image/jpg" | "image/pjpeg") {
        return Some("image/jpeg");
    }
    if matches!(content_type, "image/png" | "image/x-png") {
        return Some("image/png");
    }
    if content_type.eq_ignore_ascii_case("image/webp") {
        return Some("image/webp");
    }
    if content_type.eq_ignore_ascii_case("image/gif") {
        return Some("image/gif");
    }
    if content_type.eq_ignore_ascii_case("image/heic") {
        return Some("image/heic");
    }
    if content_type.eq_ignore_ascii_case("image/heif") {
        return Some("image/heif");
    }
    None
}

#[derive(Debug, thiserror::Error)]
enum ImageFetchError {
    #[error("image request failed")]
    Request(#[from] reqwest::Error),
    #[error("image target resolution failed")]
    Resolution(#[source] std::io::Error),
    #[error("image request timed out")]
    Timeout,
    #[error("image target is unsafe")]
    UnsafeTarget,
    #[error("image response is unsuccessful")]
    Response,
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

impl ImageFetchError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Request(_) | Self::Resolution(_) | Self::Timeout | Self::Response
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, net::IpAddr, sync::Arc};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
        task::JoinHandle,
    };

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
            Err(ImageFetchError::UnsafeTarget)
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
            Err(ImageFetchError::UnsafeTarget)
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
            Err(ImageFetchError::BodyTooLarge)
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

        assert!(matches!(
            fetcher.fetch_once(&server.url).await,
            Err(ImageFetchError::Request(error)) if error.is_timeout()
        ));
        assert_eq!(server.finish().await?, vec!["/".to_owned()]);
        Ok(())
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
