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
use crate::spider::utils::url::CrawledUrl;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};
use url::Url;

/// What robots.txt allows a crawl below an established root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CrawlRobots {
    pub(super) deny_list: DenyList,
    pub(super) crawl_delay: Option<Duration>,
    /// The rules themselves, to admit crawl seeds; none when robots.txt is unavailable, which
    /// allows everything.
    pub(super) rules: Option<RobotsRules>,
    /// Stable label for the crawl diagnostics.
    pub(super) policy: &'static str,
}

impl CrawlRobots {
    /// Whether robots.txt allows the crawler to request `url`.
    pub(super) fn allows(&self, url: &Url) -> bool {
        self.rules.as_ref().is_none_or(|rules| rules.allows(url))
    }
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
    let policy = match fetch_robots_policy(transport, root, root_client, timeout).await? {
        FetchedRobots::Policy(policy) => policy,
        FetchedRobots::Unreachable(unreachable) => {
            return Err(robots_unreachable_error(Some(Box::new(unreachable))));
        }
    };
    let label = policy.label();
    match policy {
        RobotsPolicy::Unreachable => Err(robots_unreachable_error(None)),
        RobotsPolicy::Unavailable => Ok(CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            rules: None,
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
        rules: Some(rules.clone()),
        policy,
    }
}

/// Why robots.txt could not be reached: the HTTP status it answered, or the kind of the
/// transport failure when it gave no answer. Neither carries a response body or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnreachableCause {
    Status(u16),
    Transport(NetworkErrorKind),
}

impl std::fmt::Display for UnreachableCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(status) => write!(f, "HTTP status {status}"),
            Self::Transport(kind) => write!(f, "transport error {kind:?}"),
        }
    }
}

/// The robots.txt a crawl treated as unreachable, in the stored URL spelling, so the failure
/// log names what was requested and what came back.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("robots.txt at {url} is unreachable: {cause}")]
pub struct RobotsUnreachable {
    url: String,
    cause: UnreachableCause,
}

impl RobotsUnreachable {
    fn new(robots_url: &Url, cause: UnreachableCause) -> Self {
        Self {
            url: CrawledUrl::new(robots_url.clone()).to_string(),
            cause,
        }
    }

    /// The robots.txt URL that was requested.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The HTTP status robots.txt answered, when it answered.
    pub fn status(&self) -> Option<u16> {
        match self.cause {
            UnreachableCause::Status(status) => Some(status),
            UnreachableCause::Transport(_) => None,
        }
    }

    /// The transport error kind, when robots.txt gave no answer.
    pub fn transport_error(&self) -> Option<&'static str> {
        match self.cause {
            UnreachableCause::Status(_) => None,
            UnreachableCause::Transport(kind) => Some(kind.as_str()),
        }
    }
}

/// The outcome of reading robots.txt; an unreachable one keeps its cause.
enum FetchedRobots {
    Policy(RobotsPolicy),
    Unreachable(RobotsUnreachable),
}

/// One robots.txt answer: what to do next, or the unreachable answer with its status.
enum RobotsStep {
    Fetched(RobotsFetchStep),
    Unreachable(RobotsUnreachable),
}

/// Redirects are followed to any public host, as RFC 9309 asks.
async fn fetch_robots_policy(
    transport: &dyn SpiderTransport,
    root: &Url,
    root_client: &spider::reqwest::Client,
    timeout: Duration,
) -> Result<FetchedRobots, SpiderDiscoveryError> {
    let mut robots_url = robots_txt_url(root);
    for hop in 0..=MAX_ROBOTS_TXT_REDIRECTS {
        match robots_txt_step(transport, root, root_client, &robots_url, timeout).await {
            Ok(RobotsStep::Fetched(RobotsFetchStep::Policy(policy))) => {
                return Ok(FetchedRobots::Policy(policy));
            }
            Ok(RobotsStep::Fetched(RobotsFetchStep::Redirect(next))) => robots_url = next,
            Ok(RobotsStep::Unreachable(unreachable)) => {
                return Ok(FetchedRobots::Unreachable(unreachable));
            }
            Err((kind, source)) => {
                let shop_host = is_same_or_www_host(root, &robots_url);
                return match RobotsPolicy::for_failed_request(kind, hop > 0, shop_host) {
                    Some(RobotsPolicy::Unreachable) => Ok(FetchedRobots::Unreachable(
                        RobotsUnreachable::new(&robots_url, UnreachableCause::Transport(kind)),
                    )),
                    Some(policy) => Ok(FetchedRobots::Policy(policy)),
                    None => Err(robots_request_failed(source)),
                };
            }
        }
    }
    Ok(FetchedRobots::Policy(RobotsPolicy::Unavailable))
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
) -> Result<RobotsStep, RobotsRequestError> {
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
        return Ok(RobotsStep::Fetched(RobotsFetchStep::Policy(
            RobotsPolicy::from_body(&body),
        )));
    }
    let location = response
        .headers()
        .get("location")
        .and_then(crate::network::policy::location_text);
    Ok(
        match RobotsFetchStep::for_unsuccessful(status.as_u16(), location, robots_url) {
            RobotsFetchStep::Policy(RobotsPolicy::Unreachable) => RobotsStep::Unreachable(
                RobotsUnreachable::new(robots_url, UnreachableCause::Status(status.as_u16())),
            ),
            step => RobotsStep::Fetched(step),
        },
    )
}

fn request_error(error: spider::reqwest::Error) -> RobotsRequestError {
    (classify_reqwest_error(&error), Box::new(error))
}

fn robots_unreachable_error(source: Option<application::error::BoxError>) -> SpiderDiscoveryError {
    SpiderDiscoveryError::Preflight {
        kind: CrawlFailureKind::ServerError,
        reason: "robots.txt is unreachable, which disallows the crawl",
        source,
        root_reroot: None,
    }
}

fn robots_request_failed(source: application::error::BoxError) -> SpiderDiscoveryError {
    SpiderDiscoveryError::Preflight {
        kind: CrawlFailureKind::ConnectError,
        reason: "robots.txt request failed",
        source: Some(source),
        root_reroot: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_name_the_robots_url_and_status_without_credentials() {
        let url = Url::parse("https://user:secret@example.com/robots.txt?utm_source=mail").unwrap();

        let unreachable = RobotsUnreachable::new(&url, UnreachableCause::Status(503));

        assert_eq!(unreachable.url(), "https://example.com/robots.txt");
        assert_eq!(unreachable.status(), Some(503));
        assert_eq!(unreachable.transport_error(), None);
        assert_eq!(
            unreachable.to_string(),
            "robots.txt at https://example.com/robots.txt is unreachable: HTTP status 503"
        );
    }

    #[test]
    fn should_name_the_transport_error_kind_when_robots_txt_gave_no_answer() {
        let url = Url::parse("https://example.com/robots.txt").unwrap();

        let unreachable =
            RobotsUnreachable::new(&url, UnreachableCause::Transport(NetworkErrorKind::Timeout));

        assert_eq!(unreachable.status(), None);
        assert_eq!(unreachable.transport_error(), Some("TIMEOUT"));
    }
}
