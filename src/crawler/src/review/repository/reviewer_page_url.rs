use crate::network::policy::url_matches_configured_domain;
use crate::spider::utils::url::CrawledUrl;
use url::Url;

/// The URL a reviewer page is checked and stored under: the fragment never reaches the shop,
/// so `…/1` and `…/1#gallery` are the same page.
pub(super) fn reviewer_page_url(url: &Url) -> Url {
    let mut url = url.clone();
    url.set_fragment(None);
    url
}

/// A reviewer page must be a plain http(s) URL on the default port of one of the ListingSource's
/// domains, so the console never probes other services on a shop's host. URLs with credentials
/// are refused so they are never fetched, logged or stored.
pub(super) fn is_allowed_reviewer_page_url(url: &Url, listing_source_domains: &[String]) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && listing_source_domains
            .iter()
            .any(|domain| url_matches_configured_domain(url, domain))
}

/// Whether a stored review page URL names the same page as `url`, under the crawler's URL
/// normalization (trailing slash, fragment, default port and similar spellings).
pub(super) fn is_same_review_page(stored_url: &str, url: &Url) -> bool {
    Url::parse(stored_url).is_ok_and(|stored_url| {
        CrawledUrl::new(stored_url).as_url() == CrawledUrl::new(url.clone()).as_url()
    })
}

pub(super) fn without_credentials(url: &Url) -> Url {
    let mut url = url.clone();
    // Both only fail for URLs without a host, which carry no credentials.
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url
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
    fn should_allow_reviewer_page_url_when_it_is_http_on_a_listing_source_domain() {
        for raw in [
            "https://example.com/products/1",
            "http://www.example.com/products/1",
        ] {
            assert!(is_allowed_reviewer_page_url(&url(raw), &domains()), "{raw}");
        }
    }

    #[test]
    fn should_refuse_reviewer_page_url_when_it_carries_credentials() {
        for raw in [
            "https://user:secret@example.com/products/1",
            "https://user@example.com/products/1",
        ] {
            assert!(
                !is_allowed_reviewer_page_url(&url(raw), &domains()),
                "{raw}"
            );
        }
    }

    #[test]
    fn should_refuse_reviewer_page_url_when_it_names_a_non_default_port() {
        for raw in [
            "http://example.com:8080/products/1",
            "https://example.com:6379/products/1",
        ] {
            assert!(
                !is_allowed_reviewer_page_url(&url(raw), &domains()),
                "{raw}"
            );
        }
        assert!(is_allowed_reviewer_page_url(
            &url("https://example.com:443/products/1"),
            &domains()
        ));
    }

    #[test]
    fn should_strip_credentials_when_reviewer_page_url_is_reported() {
        let reported = without_credentials(&url("https://user:secret@example.com/products/1"));

        assert_eq!(reported.as_str(), "https://example.com/products/1");
    }

    #[test]
    fn should_treat_review_page_as_same_when_url_differs_only_by_trailing_slash() {
        assert!(is_same_review_page(
            "https://example.com/products/1",
            &url("https://example.com/products/1/")
        ));
        assert!(!is_same_review_page(
            "https://example.com/products/1",
            &url("https://example.com/products/2")
        ));
    }

    #[test]
    fn should_drop_fragment_when_reviewer_page_url_is_normalized() {
        let normalized = reviewer_page_url(&url("https://example.com/products/1#gallery"));

        assert_eq!(normalized.as_str(), "https://example.com/products/1");
    }
}
