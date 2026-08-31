use bloomfilter::Bloom;
use reqwest::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};
use spider::page::AntiBotTech;
use spider::tokio;
use spider::utils::auto_throttle::AutoThrottleConfig;
use spider::website::{CrawlStatus, Website, WebsiteMetaInfo};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use url::Url;

use crate::network::policy::{is_same_or_www_host, resolve_public_http_target};
use crate::spider::utils::url::CrawledUrl;

const SPIDER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36";
const SPIDER_ACCEPT_ENCODING: &str = "gzip, br, deflate";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlFailureKind {
    EmptyCrawl,
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
    pub fn as_str(self) -> &'static str {
        match self {
            CrawlFailureKind::EmptyCrawl => "EmptyCrawl",
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
}

impl std::fmt::Display for CrawlFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct CrawlDiagnostics {
    pub failure_kind: Option<CrawlFailureKind>,
    pub http_status: Option<u16>,
    pub final_url: Option<String>,
    pub redirect_url: Option<String>,
    pub diagnostic_reason: Option<String>,
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
    #[error("Spider discovery error: {0}")]
    Discovery(String),
}

#[derive(Debug, Clone)]
pub struct CrawlerConfig {
    pub delay_millis: u64,
    pub request_timeout_secs: u64,
    pub concurrency_limit: usize,
    pub bloom_capacity: usize,
    pub bloom_fp_rate: f64,
    pub channel_size: usize,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            delay_millis: 500,
            request_timeout_secs: 15,
            concurrency_limit: 8,
            bloom_capacity: 100_000,
            bloom_fp_rate: 0.001,
            channel_size: 1000,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait Spider: Send + Sync {
    async fn crawl(&self, shop_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError>;
}

pub struct SpiderImpl {
    config: CrawlerConfig,
}

impl SpiderImpl {
    pub fn new(config: CrawlerConfig) -> Self {
        Self { config }
    }
}

impl Default for SpiderImpl {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

fn configured_host_whitelist(url: &Url) -> Result<String, SpiderDiscoveryError> {
    let host = url.host_str().ok_or_else(|| {
        SpiderDiscoveryError::Discovery("configured crawler URL has no host".to_string())
    })?;
    Ok(format!(
        r"^https?://{}(?::(?:80|443))?(?:/|$)",
        regex::escape(host)
    ))
}

async fn spider_public_http_client(
    url: &Url,
    timeout: std::time::Duration,
) -> Result<spider::reqwest::Client, SpiderDiscoveryError> {
    let target = resolve_public_http_target(url, timeout)
        .await
        .map_err(|error| SpiderDiscoveryError::Discovery(error.to_string()))?;
    let mut builder = spider::reqwest::Client::builder()
        .redirect(spider::reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .no_proxy();
    for address in target.addresses {
        builder = builder.resolve(&target.host, address);
    }
    builder
        .build()
        .map_err(|error| SpiderDiscoveryError::Discovery(error.to_string()))
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(&self, shop_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        let (tx, rx) = mpsc::channel(self.config.channel_size);
        let (status_tx, status_rx) = oneshot::channel();
        let (diagnostics_tx, diagnostics_rx) = oneshot::channel();

        let root_url = Url::parse(shop_url).map_err(|_| {
            SpiderDiscoveryError::Discovery("configured crawler URL is invalid".to_string())
        })?;
        let client = spider_public_http_client(
            &root_url,
            std::time::Duration::from_secs(self.config.request_timeout_secs),
        )
        .await?;
        let host_whitelist = configured_host_whitelist(&root_url)?;
        let mut website = Website::new(shop_url);
        website.set_http_client(client);

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

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_whitelist_url(Some(vec![host_whitelist.into()]))
            .with_respect_robots_txt(true)
            .with_headers(Some(spider_request_headers()))
            .with_user_agent(Some(SPIDER_USER_AGENT))
            .with_request_timeout(Some(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            )))
            .with_delay(
                std::time::Duration::from_millis(self.config.delay_millis).as_millis() as u64,
            )
            .with_caching(false)
            .build()
            .map_err(|_| SpiderDiscoveryError::Discovery("Failed to build website".to_string()))?;

        let mut spider_rx = website.subscribe(512);

        tokio::spawn(async move {
            website.crawl().await;
            let status = *website.get_status();
            let meta = *website.get_website_meta_info();
            website.unsubscribe();
            let _ = status_tx.send((status, meta));
        });

        let config = self.config.clone();
        let shop_url = shop_url.to_string();
        tokio::spawn(async move {
            let mut bloom = Bloom::new_for_fp_rate(config.bloom_capacity, config.bloom_fp_rate)
                .expect("bloom filter init failed");
            let mut diagnostics = CrawlDiagnostics::default();
            let mut first_page_seen = false;
            let configured_root = Url::parse(&shop_url).ok();
            let mut root_redirect_rejected = false;

            while let Ok(page) = spider_rx.recv().await {
                if !first_page_seen {
                    diagnostics = diagnostics_from_library_page(
                        &shop_url,
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

                let normalized = if let Ok(parsed) = url::Url::parse(raw_url) {
                    CrawledUrl::new(parsed)
                } else {
                    continue;
                };

                if root_redirect_rejected
                    || !configured_root
                        .as_ref()
                        .is_some_and(|root| is_same_or_www_host(root, normalized.as_url()))
                {
                    continue;
                }

                if normalized.is_blacklisted() {
                    continue;
                }

                let normalized_str = normalized.to_string();

                if !bloom.check(&normalized_str) {
                    bloom.set(&normalized_str);

                    if tx.send(CrawledPage { url: normalized }).await.is_err() {
                        break;
                    }
                }
            }

            if let Ok((status, meta)) = status_rx.await {
                apply_website_status(&mut diagnostics, status, meta);
            }
            let _ = diagnostics_tx.send(diagnostics);
        });

        Ok(SpiderCrawl {
            pages: rx,
            diagnostics: diagnostics_rx,
        })
    }
}

fn diagnostics_from_library_page(
    shop_url: &str,
    page_url: &str,
    status_code: u16,
    final_redirect_destination: Option<&str>,
    anti_bot_tech: AntiBotTech,
) -> CrawlDiagnostics {
    let final_url = final_redirect_destination.unwrap_or(page_url).to_string();
    let redirect_url = (final_redirect_destination != Some(page_url)).then(|| final_url.clone());
    let mut diagnostics = CrawlDiagnostics {
        http_status: Some(status_code),
        final_url: Some(final_url.clone()),
        redirect_url,
        ..CrawlDiagnostics::default()
    };

    if let Some(signal) = page_diagnostic_signal(shop_url, &final_url, status_code, anti_bot_tech) {
        diagnostics.apply_signal(signal);
    }

    diagnostics
}

fn page_diagnostic_signal(
    shop_url: &str,
    final_url: &str,
    status_code: u16,
    anti_bot_tech: AntiBotTech,
) -> Option<DiagnosticSignal> {
    anti_bot_signal(anti_bot_tech)
        .or_else(|| status_code_signal(status_code))
        .or_else(|| redirect_signal(shop_url, final_url))
}

fn anti_bot_signal(anti_bot_tech: AntiBotTech) -> Option<DiagnosticSignal> {
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

fn redirect_signal(shop_url: &str, final_url: &str) -> Option<DiagnosticSignal> {
    let original = Url::parse(shop_url).ok()?;
    let resolved = Url::parse(final_url).ok()?;

    (!is_same_or_www_host(&original, &resolved)).then_some(DiagnosticSignal::new(
        CrawlFailureKind::RedirectProblem,
        "library_redirect_to_unrelated_host",
    ))
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

    #[test]
    fn should_allow_only_configured_host_in_spider_fetch_graph() {
        let pattern =
            configured_host_whitelist(&Url::parse("https://example.com").unwrap()).unwrap();
        let whitelist = regex::Regex::new(&pattern).unwrap();

        assert!(whitelist.is_match("https://example.com/product/1"));
        assert!(!whitelist.is_match("https://www.example.com/product/1"));
        assert!(!whitelist.is_match("https://internal.example.com/product/1"));
        assert!(!whitelist.is_match("https://example.com.evil.test/product/1"));
        assert!(!whitelist.is_match("http://127.0.0.1/product/1"));
    }

    #[test]
    fn should_use_conservative_website_concurrency_limit_by_default() {
        let config = CrawlerConfig::default();

        assert_eq!(config.concurrency_limit, 8);
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
        let resolved = Url::parse("https://shop.example.com/").unwrap();

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
