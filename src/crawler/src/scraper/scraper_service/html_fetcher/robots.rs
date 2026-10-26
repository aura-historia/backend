//! robots.txt policies for scraper fetches, cached per origin. The pacing a
//! policy's `Crawl-delay` asks for lives in the auto-throttle.

use super::{FetchAttemptError, ReqwestHtmlFetcher};
use crate::network::policy::is_same_or_www_host;
use crate::network::robots::{
    MAX_ROBOTS_TXT_BYTES, MAX_ROBOTS_TXT_REDIRECTS, RobotsFetchStep, RobotsPolicy, robots_txt_url,
};
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
            policy = loaded.policy.label(),
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

    /// Redirects are followed to any public host, as RFC 9309 asks.
    async fn load_robots_policy(&self, url: &Url) -> Result<RobotsPolicy, FetchAttemptError> {
        let mut robots_url = robots_txt_url(url);
        for hop in 0..=MAX_ROBOTS_TXT_REDIRECTS {
            match self.robots_txt_step(&robots_url).await {
                Ok(RobotsFetchStep::Policy(policy)) => return Ok(policy),
                Ok(RobotsFetchStep::Redirect(next)) => robots_url = next,
                Err(error) => {
                    let shop_host = is_same_or_www_host(url, &robots_url);
                    return RobotsPolicy::for_failed_request(
                        error.error.kind(),
                        hop > 0,
                        shop_host,
                    )
                    .ok_or(error);
                }
            }
        }
        Ok(RobotsPolicy::Unavailable)
    }

    async fn robots_txt_step(
        &self,
        robots_url: &Url,
    ) -> Result<RobotsFetchStep, FetchAttemptError> {
        // robots.txt is small: it keeps the base timeout however slowly the
        // shop's pages answer.
        let response = self
            .http_client_for(robots_url, self.attempt_timeout)
            .await?
            .get(robots_url.clone())
            .send()
            .await
            .map_err(super::reqwest_fetch_error)?;
        let status = response.status();
        debug!(robots_url = %robots_url, status = status.as_u16(), "Fetched robots.txt");
        if status.is_success() {
            let body = read_robots_body(response).await?;
            return Ok(RobotsFetchStep::Policy(RobotsPolicy::from_body(&body)));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok());
        Ok(RobotsFetchStep::for_unsuccessful(
            status.as_u16(),
            location,
            robots_url,
        ))
    }
}

async fn read_robots_body(mut response: reqwest::Response) -> Result<Vec<u8>, FetchAttemptError> {
    let mut bytes = Vec::new();
    while bytes.len() < MAX_ROBOTS_TXT_BYTES {
        let Some(chunk) = response.chunk().await.map_err(super::reqwest_fetch_error)? else {
            break;
        };
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
