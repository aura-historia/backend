//! Which URLs belong to a review's ListingSource: the only URLs the console fetches for a review
//! page, a sample label, an agent's live fetch or a page inspection, and the only URLs the review
//! MCP server passes on. The operator contract is in `docs/crawler/operations.md` (review agents).

use crate::network::policy::url_on_crawl_host;
use url::Url;

/// Whether `url` is a plain http(s) URL on the default port of the ListingSource's `crawl_host`,
/// so a reviewer or an agent cannot point the console at other services on a shop's host. URLs
/// with credentials are refused so they are never fetched, logged or stored, and a source without
/// a crawl host has no such URL.
pub fn is_review_source_url(url: &Url, crawl_host: Option<&str>) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && crawl_host.is_some_and(|crawl_host| url_on_crawl_host(url, crawl_host))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRAWL_HOST: Option<&str> = Some("example.com");

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn should_allow_a_url_when_it_is_http_on_the_crawl_host() {
        for raw in [
            "https://example.com/products/1",
            "http://www.example.com/products/1",
        ] {
            assert!(is_review_source_url(&url(raw), CRAWL_HOST), "{raw}");
        }
    }

    #[test]
    fn should_refuse_every_url_when_the_source_has_no_crawl_host() {
        assert!(!is_review_source_url(
            &url("https://example.com/products/1"),
            None
        ));
    }

    #[test]
    fn should_refuse_a_url_when_it_carries_credentials() {
        for raw in [
            "https://user:secret@example.com/products/1",
            "https://user@example.com/products/1",
        ] {
            assert!(!is_review_source_url(&url(raw), CRAWL_HOST), "{raw}");
        }
    }

    #[test]
    fn should_refuse_a_url_when_it_names_a_non_default_port() {
        for raw in [
            "http://example.com:8080/products/1",
            "https://example.com:6379/products/1",
        ] {
            assert!(!is_review_source_url(&url(raw), CRAWL_HOST), "{raw}");
        }
        assert!(is_review_source_url(
            &url("https://example.com:443/products/1"),
            CRAWL_HOST
        ));
    }
}
