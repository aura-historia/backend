//! Redirects inside a crawl (`docs/crawler/networking.md#destination-policy`).
//!
//! Spider's client never follows a redirect itself: each hop must pass the crawl's own rules, and
//! the client is pinned to the crawl root's addresses. So a page that answers 3xx reaches the
//! relay empty, and the relay hands its `Location` target to Spider's own queue
//! ([`Website::queue`](spider::website::Website::queue)). Spider admits a queued link the way it
//! admits a found one: by its visited set, host whitelist and deny list. A twin spelling the
//! crawl knows or already fetched is therefore not requested again, and one it has not is.

use std::collections::HashMap;

use spider::reqwest::header::{HeaderMap, LOCATION};
use tokio::sync::broadcast;
use url::Url;

use super::frontier::FoundLinks;
use super::robots::CrawlRobots;
use crate::network::policy::{location_text, validate_public_http_url};

/// Redirects one page may be the end of a chain of, as many as the crawl root's own chain.
pub(super) const MAX_REDIRECT_HOPS: usize = 5;

/// What the relay learned from a delivered page about redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RedirectAnswer {
    NotARedirect,
    /// The target was queued; the redirecting page holds nothing to store.
    Followed,
    /// The target is off the crawl, refused by a rule or past the hop bound.
    Rejected,
}

/// Follows the redirects the crawl's pages answer, to targets on the crawl root's host alone
/// (Spider's whitelist): the client is pinned to that host's addresses, and a twin host would be
/// resolved unpinned.
pub(super) struct RedirectFollower {
    queue: broadcast::Sender<String>,
    robots: CrawlRobots,
    /// How many redirects led to each queued target.
    hops: HashMap<String, usize>,
}

impl RedirectFollower {
    pub(super) fn new(queue: broadcast::Sender<String>, robots: CrawlRobots) -> Self {
        Self {
            queue,
            robots,
            hops: HashMap::new(),
        }
    }

    pub(super) fn answer(
        &mut self,
        status: u16,
        page_url: &str,
        headers: Option<&HeaderMap>,
        found: &FoundLinks,
    ) -> RedirectAnswer {
        if !matches!(status, 301 | 302 | 303 | 307 | 308) {
            return RedirectAnswer::NotARedirect;
        }
        let Some(target) = self.target(page_url, headers, found) else {
            return RedirectAnswer::Rejected;
        };
        // No receiver means Spider's crawl has ended; the target waits in the found links.
        let _ = self.queue.send(target);
        RedirectAnswer::Followed
    }

    /// The stored spelling of the page's redirect target, when the crawl follows it.
    fn target(
        &mut self,
        page_url: &str,
        headers: Option<&HeaderMap>,
        found: &FoundLinks,
    ) -> Option<String> {
        let location = location_text(headers?.get(LOCATION)?)?;
        let resolved = Url::parse(page_url).ok()?.join(location.trim()).ok()?;
        let hop = self.hops.get(page_url).copied().unwrap_or(0) + 1;
        if hop > MAX_REDIRECT_HOPS || validate_public_http_url(&resolved).is_err() {
            return None;
        }
        let stored = found.redirect_target(resolved)?;
        let target = stored.as_url().as_str().to_string();
        if !self.robots.allows(stored.as_url()) || target == page_url {
            return None;
        }
        found.record_redirect_target(&stored);
        self.hops.insert(target.clone(), hop);
        Some(target)
    }
}
