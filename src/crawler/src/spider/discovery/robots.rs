//! robots.txt for a Spider crawl, through the crawler's shared RFC 9309
//! rules. The Spider library's own robots handling stays off: its regex mode
//! ignores the group naming the crawler and matches paths unanchored. Spider
//! offers no pre-fetch hook besides its URL deny list, so the rules reach it
//! as deny-list patterns.

use super::website_spider::{CrawlFailureKind, SpiderDiscoveryError, SpiderTransport};
use crate::network::policy::is_same_or_www_host;
use crate::network::robots::{DenyList, MAX_ROBOTS_TXT_BYTES, RobotsPolicy, RobotsRules};
use std::time::Duration;
use tracing::{debug, info};
use url::Url;

/// RFC 9309 asks crawlers to follow at least five consecutive redirects.
const ROBOTS_MAX_REDIRECTS: usize = 5;

/// What robots.txt allows a crawl below an established root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CrawlRobots {
    pub(super) deny_list: DenyList,
    pub(super) crawl_delay: Option<Duration>,
    /// Stable label for the crawl diagnostics.
    pub(super) policy: &'static str,
}

/// Fetches robots.txt for `root` and admits the crawl. A root robots.txt
/// disallows, or a robots.txt that cannot be read, fails the crawl before
/// Spider fetches anything else.
pub(super) async fn admit_crawl(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    timeout: Duration,
) -> Result<CrawlRobots, SpiderDiscoveryError> {
    let policy = fetch_robots_policy(transport, root, root_client, timeout).await?;
    match policy {
        RobotsPolicy::Unreachable => Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::ServerError,
            "robots.txt is unreachable, which disallows the crawl",
        )),
        RobotsPolicy::Unavailable => Ok(CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            policy: "unavailable_allow_all",
        }),
        RobotsPolicy::Rules(rules) if !rules.allows(root) => Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::RobotsBlocked,
            "robots.txt disallows the crawl root",
        )),
        RobotsPolicy::Rules(rules) => Ok(crawl_robots(root, &rules)),
    }
}

fn crawl_robots(root: &Url, rules: &RobotsRules) -> CrawlRobots {
    let deny_list = rules.deny_list();
    if !deny_list.failing_closed.is_empty() {
        info!(
            event = "crawler.spider.robots_rules_failing_closed",
            crawl_root = %root,
            disallow_patterns = ?deny_list.failing_closed,
            "Spider skips URLs these robots.txt Disallow rules cover although an Allow exception permits them"
        );
    }
    CrawlRobots {
        deny_list,
        crawl_delay: rules.crawl_delay(),
        policy: "rules",
    }
}

/// Requests to the root host reuse the client pinned to it in preflight; a
/// redirect to the bare or `www` sibling gets its own validated client.
async fn fetch_robots_policy(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    timeout: Duration,
) -> Result<RobotsPolicy, SpiderDiscoveryError> {
    let mut robots_url = root.clone();
    robots_url.set_path("/robots.txt");
    robots_url.set_query(None);
    robots_url.set_fragment(None);
    for _ in 0..=ROBOTS_MAX_REDIRECTS {
        let client = if robots_url.host() == root.host() {
            root_client.clone()
        } else {
            transport
                .client_for(&robots_url, timeout)
                .await
                .map_err(|error| robots_request_failed(Box::new(error)))?
        };
        let mut response = client
            .get(robots_url.clone())
            .send()
            .await
            .map_err(|error| robots_request_failed(Box::new(error)))?;
        let status = response.status();
        debug!(robots_url = %robots_url, status = status.as_u16(), "Fetched robots.txt for spider");
        if status.is_success() {
            let mut body = Vec::new();
            while body.len() < MAX_ROBOTS_TXT_BYTES {
                match response.chunk().await {
                    Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                    Ok(None) => break,
                    Err(error) => return Err(robots_request_failed(Box::new(error))),
                }
            }
            return Ok(RobotsPolicy::Rules(RobotsRules::parse(
                &String::from_utf8_lossy(&body),
            )));
        }
        if !status.is_redirection() {
            return Ok(RobotsPolicy::for_failed_status(status.as_u16()));
        }
        let next = response
            .headers()
            .get("location")
            .and_then(|location| location.to_str().ok())
            .and_then(|location| robots_url.join(location).ok());
        match next {
            Some(next) if is_same_or_www_host(root, &next) => robots_url = next,
            // No robots.txt on the shop's host: RFC 9309 treats it as
            // unavailable.
            _ => return Ok(RobotsPolicy::Unavailable),
        }
    }
    Ok(RobotsPolicy::Unavailable)
}

fn robots_request_failed(source: application::error::BoxError) -> SpiderDiscoveryError {
    SpiderDiscoveryError::Preflight {
        kind: CrawlFailureKind::ConnectError,
        reason: "robots.txt request failed",
        source: Some(source),
    }
}
