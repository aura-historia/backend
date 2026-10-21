pub mod policy;
mod tls;

/// The one user agent every crawler request sends: spider pages and
/// robots.txt, scraper HTML fetches and image probes.
///
/// It identifies the crawler honestly instead of posing as a browser. Spider
/// matches robots.txt groups against the token before the first `/`, so shops
/// can address `AuraHistoriaBot` directly. The bare token form is deliberate:
/// some shop firewalls refuse the `Mozilla/5.0 (compatible; ...)` form from
/// unverified addresses, and a pinned browser version goes stale and gets
/// blocked as outdated. The version is the crate's, so a shop's log entry
/// maps to a build.
pub const CRAWLER_USER_AGENT: &str = concat!(
    "AuraHistoriaBot/",
    env!("CARGO_PKG_VERSION"),
    " (+https://aura-historia.com)"
);
