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
    DenyList, MAX_ROBOTS_TXT_BYTES, MAX_ROBOTS_TXT_REDIRECTS, RobotsFetchStep, RobotsPolicy,
    RobotsRules, robots_txt_url,
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
    let label = policy.label();
    match policy {
        RobotsPolicy::Unreachable => Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::ServerError,
            "robots.txt is unreachable, which disallows the crawl",
        )),
        RobotsPolicy::Unavailable => Ok(CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            policy: label,
        }),
        RobotsPolicy::Rules(rules) if !rules.allows(root) => Err(SpiderDiscoveryError::preflight(
            CrawlFailureKind::RobotsBlocked,
            "robots.txt disallows the crawl root",
        )),
        RobotsPolicy::Rules(rules) => Ok(crawl_robots(root, &rules, label)),
    }
}

fn crawl_robots(root: &Url, rules: &RobotsRules, policy: &'static str) -> CrawlRobots {
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
        policy,
    }
}

/// Redirects are followed to any public host, as RFC 9309 asks.
async fn fetch_robots_policy(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    timeout: Duration,
) -> Result<RobotsPolicy, SpiderDiscoveryError> {
    let mut robots_url = robots_txt_url(root);
    for hop in 0..=MAX_ROBOTS_TXT_REDIRECTS {
        match robots_txt_step(transport, root, root_client, &robots_url, timeout).await {
            Ok(RobotsFetchStep::Policy(policy)) => return Ok(policy),
            Ok(RobotsFetchStep::Redirect(next)) => robots_url = next,
            Err((kind, source)) => {
                let shop_host = is_same_or_www_host(root, &robots_url);
                return RobotsPolicy::for_failed_request(kind, hop > 0, shop_host)
                    .ok_or_else(|| robots_request_failed(source));
            }
        }
    }
    Ok(RobotsPolicy::Unavailable)
}

type RobotsRequestError = (NetworkErrorKind, application::error::BoxError);

/// One robots.txt request. Requests to the root's origin reuse the client
/// pinned to it in preflight; any other target is validated first.
async fn robots_txt_step(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    robots_url: &Url,
    timeout: Duration,
) -> Result<RobotsFetchStep, RobotsRequestError> {
    let client = if robots_url.origin() == root.origin() {
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
    let mut response = client
        .get(robots_url.clone())
        .send()
        .await
        .map_err(request_error)?;
    let status = response.status();
    debug!(robots_url = %robots_url, status = status.as_u16(), "Fetched robots.txt for spider");
    if status.is_success() {
        let mut body = Vec::new();
        while body.len() < MAX_ROBOTS_TXT_BYTES {
            match response.chunk().await.map_err(request_error)? {
                Some(chunk) => body.extend_from_slice(&chunk),
                None => break,
            }
        }
        return Ok(RobotsFetchStep::Policy(RobotsPolicy::from_body(&body)));
    }
    let location = response
        .headers()
        .get("location")
        .and_then(|location| location.to_str().ok());
    Ok(RobotsFetchStep::for_unsuccessful(
        status.as_u16(),
        location,
        robots_url,
    ))
}

fn request_error(error: spider::reqwest::Error) -> RobotsRequestError {
    (classify_reqwest_error(&error), Box::new(error))
}

fn robots_request_failed(source: application::error::BoxError) -> SpiderDiscoveryError {
    SpiderDiscoveryError::Preflight {
        kind: CrawlFailureKind::ConnectError,
        reason: "robots.txt request failed",
        source: Some(source),
    }
}
