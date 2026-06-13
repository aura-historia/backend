use bloomfilter::Bloom;
use reqwest::redirect::Policy;
use spider::tokio;
use spider::website::Website;
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, warn};
use url::Url;

use crate::spider::utils::url::CrawledUrl;

const SPIDER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36";

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlFailureKind {
    RateLimited,
    AccessDenied,
    CloudflareChallenge,
    TlsError,
    RobotsBlocked,
    RedirectProblem,
    JavascriptRequired,
}

impl CrawlFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CrawlFailureKind::RateLimited => "RateLimited",
            CrawlFailureKind::AccessDenied => "AccessDenied",
            CrawlFailureKind::CloudflareChallenge => "CloudflareChallenge",
            CrawlFailureKind::TlsError => "TlsError",
            CrawlFailureKind::RobotsBlocked => "RobotsBlocked",
            CrawlFailureKind::RedirectProblem => "RedirectProblem",
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

#[derive(Debug)]
pub struct SpiderCrawl {
    pub pages: mpsc::Receiver<CrawledPage>,
    pub diagnostics: CrawlDiagnostics,
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
    pub bloom_capacity: usize,
    pub bloom_fp_rate: f64,
    pub channel_size: usize,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            delay_millis: 500,
            request_timeout_secs: 15,
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

struct CrawlSeedResolution {
    seed_url: String,
    diagnostics: CrawlDiagnostics,
}

impl SpiderImpl {
    pub fn new(config: CrawlerConfig) -> Self {
        Self { config }
    }

    async fn resolve_crawl_seed_url(&self, shop_url: &str) -> CrawlSeedResolution {
        let mut diagnostics = CrawlDiagnostics::default();

        let Ok(original_url) = Url::parse(shop_url) else {
            warn!(
                shop_url,
                "Skipping canonical URL resolution for invalid crawl seed"
            );
            diagnostics.diagnostic_reason = Some("invalid_crawl_seed".to_string());
            return CrawlSeedResolution {
                seed_url: shop_url.to_string(),
                diagnostics,
            };
        };

        let client = match reqwest::Client::builder()
            .redirect(Policy::limited(10))
            .user_agent(SPIDER_USER_AGENT)
            .timeout(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            ))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                warn!(shop_url, error = ?error, "Failed to build canonical URL resolver");
                diagnostics.diagnostic_reason = Some("canonical_resolver_build_failed".to_string());
                return CrawlSeedResolution {
                    seed_url: shop_url.to_string(),
                    diagnostics,
                };
            }
        };

        populate_robots_diagnostics(&client, &original_url, &mut diagnostics).await;

        let response = match client.get(original_url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                if is_tls_error(&error) {
                    diagnostics.failure_kind = Some(CrawlFailureKind::TlsError);
                    diagnostics.diagnostic_reason = Some("tls_error".to_string());
                } else {
                    diagnostics.diagnostic_reason = Some("canonical_request_failed".to_string());
                }
                warn!(shop_url, error = ?error, "Canonical URL resolution failed");
                return CrawlSeedResolution {
                    seed_url: shop_url.to_string(),
                    diagnostics,
                };
            }
        };

        diagnostics.http_status = Some(response.status().as_u16());
        diagnostics.final_url = Some(response.url().to_string());

        if !response.status().is_success() {
            diagnostics.failure_kind = failure_kind_for_status(response.status().as_u16());
            diagnostics.diagnostic_reason = Some("canonical_non_success_status".to_string());
            warn!(
                shop_url,
                status = %response.status(),
                "Canonical URL resolution returned non-success status"
            );
            return CrawlSeedResolution {
                seed_url: shop_url.to_string(),
                diagnostics,
            };
        }

        let resolved_url = response.url().clone();
        diagnostics.redirect_url = (original_url != resolved_url).then(|| resolved_url.to_string());

        if !is_same_or_www_host(&original_url, &resolved_url) {
            diagnostics.failure_kind = Some(CrawlFailureKind::RedirectProblem);
            diagnostics.diagnostic_reason = Some("redirect_to_unrelated_host".to_string());
            warn!(
                original_url = %original_url,
                resolved_url = %resolved_url,
                "Ignoring canonical URL redirect to unrelated host"
            );
            return CrawlSeedResolution {
                seed_url: shop_url.to_string(),
                diagnostics,
            };
        }

        if diagnostics.failure_kind.is_none() {
            match response.text().await {
                Ok(body) => populate_html_diagnostics(&body, &mut diagnostics),
                Err(error) => {
                    diagnostics.diagnostic_reason = Some("homepage_body_read_failed".to_string());
                    warn!(shop_url, error = ?error, "Failed to read homepage body for diagnostics");
                }
            }
        }

        if original_url != resolved_url {
            debug!(
                original_url = %original_url,
                resolved_url = %resolved_url,
                host_normalized = original_url.host_str() != resolved_url.host_str(),
                "Resolved canonical crawl URL"
            );
        }

        CrawlSeedResolution {
            seed_url: resolved_url.to_string(),
            diagnostics,
        }
    }
}

impl Default for SpiderImpl {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(&self, shop_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        let (tx, rx) = mpsc::channel(self.config.channel_size);

        let seed_resolution = self.resolve_crawl_seed_url(shop_url).await;
        let mut website = Website::new(&seed_resolution.seed_url);

        let blacklist_regex = CrawledUrl::blacklist_patterns();

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_respect_robots_txt(true)
            .with_user_agent(Some(SPIDER_USER_AGENT))
            .with_request_timeout(Some(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            )))
            .with_delay(
                std::time::Duration::from_millis(self.config.delay_millis).as_millis() as u64,
            )
            .with_caching(false)
            .build()
            .map_err(|e| SpiderDiscoveryError::Discovery(e.to_string()))?;

        let mut spider_rx = website
            .subscribe(512)
            .ok_or_else(|| SpiderDiscoveryError::Discovery("Failed to subscribe".to_string()))?;

        tokio::spawn(async move {
            website.crawl().await;
            website.unsubscribe();
        });

        let config = self.config.clone();
        tokio::spawn(async move {
            let mut bloom = Bloom::new_for_fp_rate(config.bloom_capacity, config.bloom_fp_rate)
                .expect("bloom filter init failed");

            while let Ok(page) = spider_rx.recv().await {
                let raw_url = page.get_url();

                let normalized = if let Ok(parsed) = url::Url::parse(raw_url) {
                    CrawledUrl::new(parsed)
                } else {
                    continue;
                };

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
        });

        Ok(SpiderCrawl {
            pages: rx,
            diagnostics: seed_resolution.diagnostics,
        })
    }
}

fn failure_kind_for_status(status: u16) -> Option<CrawlFailureKind> {
    match status {
        429 => Some(CrawlFailureKind::RateLimited),
        403 => Some(CrawlFailureKind::AccessDenied),
        _ => None,
    }
}

fn is_tls_error(error: &reqwest::Error) -> bool {
    let text = format!("{error:?}").to_ascii_lowercase();
    text.contains("certificate")
        || text.contains("cert")
        || text.contains("tls")
        || text.contains("ssl")
        || text.contains("schannel")
}

async fn populate_robots_diagnostics(
    client: &reqwest::Client,
    original_url: &Url,
    diagnostics: &mut CrawlDiagnostics,
) {
    let Ok(robots_url) = original_url.join("/robots.txt") else {
        return;
    };
    let Ok(response) = client.get(robots_url).send().await else {
        return;
    };
    if !response.status().is_success() {
        return;
    }
    let Ok(body) = response.text().await else {
        return;
    };
    if robots_disallows_root_for_all_agents(&body) {
        diagnostics.failure_kind = Some(CrawlFailureKind::RobotsBlocked);
        diagnostics.diagnostic_reason = Some("robots_disallow_root".to_string());
    }
}

fn robots_disallows_root_for_all_agents(body: &str) -> bool {
    let mut applies_to_all = false;
    for raw_line in body.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        match name.as_str() {
            "user-agent" => applies_to_all = value == "*",
            "disallow" if applies_to_all && value == "/" => return true,
            _ => {}
        }
    }
    false
}

fn populate_html_diagnostics(body: &str, diagnostics: &mut CrawlDiagnostics) {
    let lower = body.to_ascii_lowercase();
    if lower.contains("cf-ray")
        || lower.contains("cf-chl")
        || lower.contains("checking your browser")
        || lower.contains("just a moment")
        || lower.contains("cloudflare")
    {
        diagnostics.failure_kind = Some(CrawlFailureKind::CloudflareChallenge);
        diagnostics.diagnostic_reason = Some("cloudflare_challenge_html".to_string());
        return;
    }

    let link_count = lower.matches("href=").count();
    let looks_app_rendered = lower.contains("__next")
        || lower.contains("id=\"root\"")
        || lower.contains("id=\"app\"")
        || lower.contains("window.__")
        || lower.contains("data-reactroot");

    if link_count < 2 && looks_app_rendered {
        diagnostics.failure_kind = Some(CrawlFailureKind::JavascriptRequired);
        diagnostics.diagnostic_reason = Some("few_links_and_app_shell_markers".to_string());
    }
}

fn is_same_or_www_host(original_url: &Url, resolved_url: &Url) -> bool {
    let Some(original_host) = original_url.host_str() else {
        return false;
    };
    let Some(resolved_host) = resolved_url.host_str() else {
        return false;
    };

    strip_www(original_host).eq_ignore_ascii_case(strip_www(resolved_host))
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn should_classify_429_as_rate_limited() {
        assert_eq!(
            failure_kind_for_status(429),
            Some(CrawlFailureKind::RateLimited)
        );
    }

    #[test]
    fn should_classify_403_as_access_denied() {
        assert_eq!(
            failure_kind_for_status(403),
            Some(CrawlFailureKind::AccessDenied)
        );
    }

    #[test]
    fn should_detect_cloudflare_challenge_html() {
        let mut diagnostics = CrawlDiagnostics::default();

        populate_html_diagnostics(
            "<html><title>Just a moment...</title><span>cf-ray</span></html>",
            &mut diagnostics,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::CloudflareChallenge)
        );
        assert_eq!(
            diagnostics.diagnostic_reason.as_deref(),
            Some("cloudflare_challenge_html")
        );
    }

    #[test]
    fn should_detect_javascript_required_app_shell() {
        let mut diagnostics = CrawlDiagnostics::default();

        populate_html_diagnostics(
            r#"<html><body><div id="root"></div><script src="/app.js"></script></body></html>"#,
            &mut diagnostics,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::JavascriptRequired)
        );
    }

    #[test]
    fn should_detect_robots_disallow_root_for_all_agents() {
        assert!(robots_disallows_root_for_all_agents(
            "User-agent: *\nDisallow: /\n"
        ));
    }

    #[test]
    fn should_not_detect_robots_block_when_only_specific_paths_are_disallowed() {
        assert!(!robots_disallows_root_for_all_agents(
            "User-agent: *\nDisallow: /cart\nAllow: /\n"
        ));
    }
}
