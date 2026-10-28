//! Which URLs belong to a review's ListingSource: the only URLs the console fetches for a review
//! page, a sample label, an agent's live fetch or a page inspection, and the only URLs the review
//! MCP server passes on. The operator contract is in `docs/crawler/operations.md` (review agents).

use crate::network::policy::url_matches_configured_domain;
use url::Url;

/// Whether `url` is a plain http(s) URL on the default port of one of the ListingSource's
/// `domains`, so a reviewer or an agent cannot point the console at other services on a shop's
/// host. URLs with credentials are refused so they are never fetched, logged or stored.
pub fn is_review_source_url(url: &Url, domains: &[String]) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && domains
            .iter()
            .any(|domain| url_matches_configured_domain(url, domain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains() -> Vec<String> {
        vec!["example.com".to_string()]
    }

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn should_allow_a_url_when_it_is_http_on_a_listing_source_domain() {
        for raw in [
            "https://example.com/products/1",
            "http://www.example.com/products/1",
        ] {
            assert!(is_review_source_url(&url(raw), &domains()), "{raw}");
        }
    }

    #[test]
    fn should_refuse_a_url_when_it_carries_credentials() {
        for raw in [
            "https://user:secret@example.com/products/1",
            "https://user@example.com/products/1",
        ] {
            assert!(!is_review_source_url(&url(raw), &domains()), "{raw}");
        }
    }

    #[test]
    fn should_refuse_a_url_when_it_names_a_non_default_port() {
        for raw in [
            "http://example.com:8080/products/1",
            "https://example.com:6379/products/1",
        ] {
            assert!(!is_review_source_url(&url(raw), &domains()), "{raw}");
        }
        assert!(is_review_source_url(
            &url("https://example.com:443/products/1"),
            &domains()
        ));
    }
}
