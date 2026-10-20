//! robots.txt state for scraper fetches: the policy cached per origin, and the
//! time of the last request per crawler domain, which `Crawl-delay` is
//! measured from.

use super::{FetchAttemptError, ReqwestHtmlFetcher};
use crate::network::policy::{canonical_crawler_domain, is_same_or_www_host, redirect_target};
use crate::network::robots::{MAX_ROBOTS_TXT_BYTES, RobotsPolicy, RobotsRules};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info};
use url::Url;

/// How long a fetched robots.txt (or a 4xx answer) is trusted; RFC 9309
/// allows at most 24 hours.
const ROBOTS_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
/// How long an unreachable robots.txt disallows its origin before the next
/// request fetches it again.
const ROBOTS_UNREACHABLE_RETRY: Duration = Duration::from_secs(15 * 60);
/// RFC 9309 asks crawlers to follow at least five consecutive redirects.
const ROBOTS_MAX_REDIRECTS: usize = 5;

#[derive(Default)]
pub(super) struct RobotsCache {
    policies: DashMap<String, (Arc<RobotsPolicy>, Instant)>,
    /// Keyed by crawler domain: a shop's bare and `www` hosts share one
    /// request spacing, as they share one scraper domain.
    last_requests: DashMap<String, Instant>,
}

impl RobotsCache {
    fn cached(&self, origin: &str) -> Option<Arc<RobotsPolicy>> {
        let entry = self.policies.get(origin)?;
        let (policy, expires_at) = entry.value();
        (Instant::now() < *expires_at).then(|| Arc::clone(policy))
    }

    fn store(&self, origin: &str, policy: RobotsPolicy) -> Arc<RobotsPolicy> {
        let ttl = match policy {
            RobotsPolicy::Unreachable => ROBOTS_UNREACHABLE_RETRY,
            RobotsPolicy::Rules(_) | RobotsPolicy::Unavailable => ROBOTS_CACHE_TTL,
        };
        let policy = Arc::new(policy);
        self.policies.insert(
            origin.to_owned(),
            (Arc::clone(&policy), Instant::now() + ttl),
        );
        policy
    }

    /// The part of `crawl_delay` that has not yet passed since the last
    /// request to `url`'s crawler domain.
    pub(super) fn crawl_delay_remaining(&self, url: &Url, crawl_delay: Duration) -> Duration {
        self.last_requests
            .get(&crawler_domain(url))
            .map_or(Duration::ZERO, |last| {
                crawl_delay.saturating_sub(last.elapsed())
            })
    }

    pub(super) fn record_request(&self, url: &Url) {
        self.last_requests
            .insert(crawler_domain(url), Instant::now());
    }
}

fn crawler_domain(url: &Url) -> String {
    canonical_crawler_domain(url.host_str().unwrap_or_default())
}

fn origin_key(url: &Url) -> String {
    url.origin().ascii_serialization()
}

impl ReqwestHtmlFetcher {
    /// The robots.txt policy for `url`'s origin, fetched once per cache
    /// lifetime. A transport failure is returned instead of cached so the
    /// domain circuit sees it like any other failed request.
    pub(super) async fn robots_policy(
        &self,
        url: &Url,
    ) -> Result<Arc<RobotsPolicy>, FetchAttemptError> {
        let origin = origin_key(url);
        if let Some(policy) = self.robots.cached(&origin) {
            return Ok(policy);
        }
        let policy = self.load_robots_policy(url).await?;
        info!(
            event = "crawler.scraper.robots_loaded",
            origin = %origin,
            policy = robots_policy_label(&policy),
            crawl_delay_ms = policy.crawl_delay().map(|delay| delay.as_millis() as u64),
            "Loaded robots.txt for scraper origin"
        );
        Ok(self.robots.store(&origin, policy))
    }

    async fn load_robots_policy(&self, url: &Url) -> Result<RobotsPolicy, FetchAttemptError> {
        let mut robots_url = url.clone();
        robots_url.set_path("/robots.txt");
        robots_url.set_query(None);
        robots_url.set_fragment(None);
        for _ in 0..=ROBOTS_MAX_REDIRECTS {
            let client = self.http_client_for(&robots_url).await?;
            let response = client
                .get(robots_url.clone())
                .send()
                .await
                .map_err(super::reqwest_fetch_error)?;
            let status = response.status();
            debug!(robots_url = %robots_url, status = status.as_u16(), "Fetched robots.txt");
            if status.is_success() {
                let body = read_robots_body(response).await?;
                return Ok(RobotsPolicy::Rules(RobotsRules::parse(&body)));
            }
            if !status.is_redirection() {
                return Ok(RobotsPolicy::for_failed_status(status.as_u16()));
            }
            match redirect_target(&robots_url, &response) {
                Ok(next) if is_same_or_www_host(url, &next) => robots_url = next,
                // A redirect off the shop's host leaves no robots.txt the
                // crawler may read: RFC 9309 treats that as unavailable.
                _ => return Ok(RobotsPolicy::Unavailable),
            }
        }
        Ok(RobotsPolicy::Unavailable)
    }
}

async fn read_robots_body(mut response: reqwest::Response) -> Result<String, FetchAttemptError> {
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ROBOTS_TXT_BYTES {
        let Some(chunk) = response.chunk().await.map_err(super::reqwest_fetch_error)? else {
            break;
        };
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn robots_policy_label(policy: &RobotsPolicy) -> &'static str {
    match policy {
        RobotsPolicy::Rules(_) => "rules",
        RobotsPolicy::Unavailable => "unavailable_allow_all",
        RobotsPolicy::Unreachable => "unreachable_disallow_all",
    }
}
