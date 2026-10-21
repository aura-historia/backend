//! robots.txt for a Spider crawl, through the crawler's shared RFC 9309
//! rules. The Spider library's own robots handling stays off: its regex mode
//! ignores the group naming the crawler and matches paths unanchored. Spider
//! offers no pre-fetch hook besides its URL deny list, so the rules reach it
//! as deny-list patterns.

use super::website_spider::{
    CrawlFailureKind, SpiderDiscoveryError, SpiderTransport, SpiderTransportError,
};
use crate::network::policy::{
    NetworkErrorKind, classify_reqwest_error, is_same_or_www_host,
    network_error_kind_for_public_target_error,
};
use crate::network::robots::{
    DenyList, MAX_ROBOTS_TXT_BYTES, MAX_ROBOTS_TXT_REDIRECTS, RobotsPolicy, RobotsRules,
    robots_txt_url,
};
use std::time::Duration;
use tracing::{debug, info};
use url::Url;

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

/// Redirects are followed to any public host, as RFC 9309 asks; each other
/// host gets its own validated client.
async fn fetch_robots_policy(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    timeout: Duration,
) -> Result<RobotsPolicy, SpiderDiscoveryError> {
    let mut robots_url = robots_txt_url(root);
    for hop in 0..=MAX_ROBOTS_TXT_REDIRECTS {
        let mut response =
            match request_robots_txt(transport, root, root_client, &robots_url, timeout).await {
                Ok(response) => response,
                Err((kind, source)) => {
                    let shop_host = is_same_or_www_host(root, &robots_url);
                    return match RobotsPolicy::for_failed_redirect(kind, shop_host) {
                        Some(policy) if hop > 0 => Ok(policy),
                        _ => Err(robots_request_failed(source)),
                    };
                }
            };
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
            return Ok(RobotsPolicy::from_body(&body));
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
            Some(next) => robots_url = next,
            // A redirect without a usable target leaves no robots.txt to
            // read: RFC 9309 treats that as unavailable.
            None => return Ok(RobotsPolicy::Unavailable),
        }
    }
    Ok(RobotsPolicy::Unavailable)
}

/// One robots.txt request. HTTP(S) requests to the root host reuse the client
/// pinned to it in preflight; any other target is validated first.
async fn request_robots_txt(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    robots_url: &Url,
    timeout: Duration,
) -> Result<spider::reqwest::Response, (NetworkErrorKind, application::error::BoxError)> {
    let client =
        if robots_url.host() == root.host() && matches!(robots_url.scheme(), "http" | "https") {
            root_client.clone()
        } else {
            transport
                .client_for(robots_url, timeout)
                .await
                .map_err(|error| {
                    let kind = match &error {
                        SpiderTransportError::Target(target) => {
                            network_error_kind_for_public_target_error(target.clone())
                        }
                        SpiderTransportError::Client(_) => NetworkErrorKind::Unknown,
                    };
                    (kind, Box::new(error) as application::error::BoxError)
                })?
        };
    client
        .get(robots_url.clone())
        .send()
        .await
        .map_err(|error| (classify_reqwest_error(&error), Box::new(error) as _))
}

fn robots_request_failed(source: application::error::BoxError) -> SpiderDiscoveryError {
    SpiderDiscoveryError::Preflight {
        kind: CrawlFailureKind::ConnectError,
        reason: "robots.txt request failed",
        source: Some(source),
    }
}
