//! robots.txt policies for scraper fetches, cached per origin. The pacing a
//! policy's `Crawl-delay` asks for lives in the auto-throttle.

use super::{FetchAttemptError, ReqwestHtmlFetcher};
use crate::network::policy::{is_same_or_www_host, redirect_target};
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
    origins: DashMap<String, Arc<OriginRobots>>,
}

/// One origin's policy. Its lock is held while robots.txt is fetched, so
/// concurrent requests to the origin wait for one fetch instead of each
/// starting their own.
#[derive(Default)]
struct OriginRobots {
    cached: tokio::sync::Mutex<Option<CachedPolicy>>,
}

struct CachedPolicy {
    policy: Arc<RobotsPolicy>,
    expires_at: Instant,
}

impl CachedPolicy {
    fn new(policy: RobotsPolicy) -> Self {
        let ttl = match policy {
            RobotsPolicy::Unreachable => ROBOTS_UNREACHABLE_RETRY,
            RobotsPolicy::Rules(_) | RobotsPolicy::Unavailable => ROBOTS_CACHE_TTL,
        };
        Self {
            policy: Arc::new(policy),
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

impl RobotsCache {
    fn origin(&self, origin: &str) -> Arc<OriginRobots> {
        Arc::clone(self.origins.entry(origin.to_owned()).or_default().value())
    }

    /// Drops expired policies no request is currently loading or reading.
    fn evict_expired(&self) {
        self.origins.retain(|_, origin| {
            Arc::strong_count(origin) > 1
                || origin.cached.try_lock().map_or(true, |cached| {
                    cached.as_ref().is_some_and(CachedPolicy::is_fresh)
                })
        });
    }
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
        let origin_key = origin_key(url);
        let origin = self.robots.origin(&origin_key);
        let mut cached = origin.cached.lock().await;
        if let Some(cached) = cached.as_ref().filter(|cached| cached.is_fresh()) {
            return Ok(Arc::clone(&cached.policy));
        }
        let loaded = CachedPolicy::new(self.load_robots_policy(url).await?);
        info!(
            event = "crawler.scraper.robots_loaded",
            origin = %origin_key,
            policy = robots_policy_label(&loaded.policy),
            crawl_delay_ms = loaded.policy.crawl_delay().map(|delay| delay.as_millis() as u64),
            "Loaded robots.txt for scraper origin"
        );
        let policy = Arc::clone(&loaded.policy);
        *cached = Some(loaded);
        drop(cached);
        drop(origin);
        self.robots.evict_expired();
        Ok(policy)
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
