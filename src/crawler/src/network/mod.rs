use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};

pub mod bot_challenge;
pub mod policy;
pub mod politeness;
pub mod robots;
pub mod robots_directives;
mod tls;

/// The one user agent every crawler request sends: spider pages and
/// robots.txt, scraper HTML fetches and image probes.
///
/// It identifies the crawler honestly instead of posing as a browser. robots.txt
/// groups are matched against the token before the first `/`, so shops can
/// address `AuraHistoriaBot` directly. The bare token form is deliberate:
/// some shop firewalls refuse the `Mozilla/5.0 (compatible; ...)` form from
/// unverified addresses, and a pinned browser version goes stale and gets
/// blocked as outdated. The version is the crate's, so a shop's log entry
/// maps to a build.
pub const CRAWLER_USER_AGENT: &str = concat!(
    "AuraHistoriaBot/",
    env!("CARGO_PKG_VERSION"),
    " (+https://aura-historia.com)"
);

/// The media types every crawler request accepts.
pub const CRAWLER_ACCEPT: &str = "*/*";

/// The headers every crawler request sends besides [`CRAWLER_USER_AGENT`]: spider pages and
/// robots.txt, scraper HTML fetches and robots.txt, image probes, and review live inspection
/// through the scraper's fetcher. Spider and scraper send the same set, so the scraper fetches
/// the site the spider learned the URL patterns from. Only the transfer encodings differ, since
/// each client advertises the ones it decodes (`docs/crawler/networking.md#request-headers`).
pub fn crawler_request_headers() -> HeaderMap {
    HeaderMap::from_iter([(ACCEPT, HeaderValue::from_static(CRAWLER_ACCEPT))])
}
