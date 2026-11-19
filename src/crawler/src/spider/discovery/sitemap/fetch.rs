//! Sends a crawl's sitemap requests through its fetch path: the client pinned to the crawl root
//! in preflight (any other host is resolved and checked under the destination policy first), at
//! the crawl's delay between request starts and within its politeness group, which also hears
//! of every rate limit (`docs/crawler/networking.md#sitemaps`).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;
use tracing::info;
use url::Url;

use super::reader::{SitemapAnswer, SitemapFailureKind, SitemapFetcher};
use crate::network::policy::{
    NetworkErrorKind, classify_reqwest_error, location_text,
    network_error_kind_for_public_target_error,
};
use crate::network::politeness::{PolitenessGroups, politeness_group};
use crate::network::response_body::{ShopBodyError, read_complete_body};
use crate::spider::discovery::page_status::RateLimitSignal;
use crate::spider::discovery::website_spider::{
    SpiderTransport, SpiderTransportError, is_tls_failure,
};

/// The crawl's fetch path, for its sitemap requests.
pub(in crate::spider::discovery) struct CrawlSitemapFetcher {
    pub(in crate::spider::discovery) transport: Arc<dyn SpiderTransport>,
    pub(in crate::spider::discovery) root: Url,
    /// The client the preflight pinned to the crawl root's addresses.
    pub(in crate::spider::discovery) root_client: spider::reqwest::Client,
    pub(in crate::spider::discovery) request_timeout: Duration,
    /// The crawl's delay between request starts.
    pub(in crate::spider::discovery) delay: Duration,
    pub(in crate::spider::discovery) politeness: Arc<PolitenessGroups>,
    pub(in crate::spider::discovery) max_body_bytes: usize,
    /// When the crawl's last request started: the preflight's, then each sitemap request's.
    pub(in crate::spider::discovery) last_start: Mutex<Instant>,
}

impl CrawlSitemapFetcher {
    /// The earliest the crawl's next request may start.
    pub(in crate::spider::discovery) fn next_start(&self) -> Instant {
        *self
            .last_start
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            + self.delay
    }

    /// Waits for the crawl's delay and the host's politeness group, and takes the start.
    async fn wait_for_turn(&self, host: &str) {
        let start = self.politeness.reserve_start(host, self.next_start());
        tokio::time::sleep_until(start.start()).await;
        start.claim();
        *self
            .last_start
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    async fn client_for(&self, url: &Url) -> Result<spider::reqwest::Client, SitemapFailureKind> {
        if url.origin() == self.root.origin() {
            return Ok(self.root_client.clone());
        }
        self.transport
            .client_for(url, self.request_timeout)
            .await
            .map(|clients| clients.shop_body)
            .map_err(|error| {
                SitemapFailureKind::Transport(match error {
                    SpiderTransportError::Target(target) => {
                        network_error_kind_for_public_target_error(target)
                    }
                    SpiderTransportError::Client(_) => NetworkErrorKind::Unknown,
                })
            })
    }

    /// Reports a rate-limit answer to the host's politeness group, which pauses and slows it.
    fn report_rate_limit(&self, host: &str, signal: RateLimitSignal) {
        if let Some(backoff) = self.politeness.record_rate_limit(host, signal.retry_after) {
            info!(
                event = "crawler.spider.politeness_group_rate_limited",
                domain = %host,
                politeness_group = %politeness_group(host).key,
                status_code = signal.status,
                retry_after_seconds = signal.retry_after.map(|value| value.as_secs()),
                pause_ms = backoff.pause.as_millis(),
                spacing_ms = backoff.spacing.as_millis(),
                "Paused politeness group after a rate limit"
            );
        }
    }
}

#[async_trait::async_trait]
impl SitemapFetcher for CrawlSitemapFetcher {
    async fn fetch(&self, url: &Url) -> SitemapAnswer {
        let Some(host) = url.host_str() else {
            return SitemapAnswer::Failed(SitemapFailureKind::OffHost);
        };
        let client = match self.client_for(url).await {
            Ok(client) => client,
            Err(kind) => return SitemapAnswer::Failed(kind),
        };
        self.wait_for_turn(host).await;
        let response = match client.get(url.clone()).send().await {
            Ok(response) => response,
            Err(error) => return SitemapAnswer::Failed(transport_failure(&error)),
        };
        let status = response.status().as_u16();
        if let Some(signal) = RateLimitSignal::from_page(status, Some(response.headers())) {
            self.report_rate_limit(host, signal);
            return SitemapAnswer::RateLimited(signal);
        }
        if response.status().is_redirection() {
            return SitemapAnswer::Redirect(
                response
                    .headers()
                    .get("location")
                    .and_then(location_text)
                    .map(str::to_owned),
            );
        }
        if !response.status().is_success() {
            return SitemapAnswer::Status(status);
        }
        match read_complete_body(response, self.max_body_bytes).await {
            Ok(body) => {
                self.politeness.record_response(host);
                SitemapAnswer::Body(body)
            }
            Err(ShopBodyError::TooLarge { .. }) => {
                SitemapAnswer::Failed(SitemapFailureKind::TooLarge)
            }
            Err(ShopBodyError::Decode { .. }) => {
                SitemapAnswer::Failed(SitemapFailureKind::Malformed)
            }
            Err(ShopBodyError::Transport(error)) => {
                SitemapAnswer::Failed(transport_failure(&error))
            }
        }
    }
}

fn transport_failure(error: &spider::reqwest::Error) -> SitemapFailureKind {
    if is_tls_failure(error) {
        return SitemapFailureKind::Tls;
    }
    SitemapFailureKind::Transport(classify_reqwest_error(error))
}
