use crate::network::policy::{canonical_crawler_domain, url_matches_configured_domain};
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
/// domains, so a reviewer cannot point the console at other services on a shop's host. URLs with
/// credentials are refused so they are never fetched, logged or stored.
pub(super) fn is_allowed_reviewer_page_url(url: &Url, listing_source_domains: &[String]) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && listing_source_domains
            .iter()
            .any(|domain| url_matches_configured_domain(url, domain))
}

/// Which shop page a review page URL names: the crawler's URL normalization (trailing slash,
/// fragment, default port and similar spellings) on the crawler's ownership host, which treats
/// `www.` and the bare host as one domain, regardless of http or https.
#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct ReviewPageIdentity {
    host: Option<String>,
    path: String,
    query: Option<String>,
}

impl ReviewPageIdentity {
    pub(super) fn of(url: &Url) -> Self {
        let normalized = CrawledUrl::new(url.clone());
        let normalized = normalized.as_url();
        Self {
            host: normalized.host_str().map(canonical_crawler_domain),
            path: normalized.path().to_owned(),
            query: normalized.query().map(str::to_owned),
        }
    }

    pub(super) fn of_stored(stored_url: &str) -> Option<Self> {
        Url::parse(stored_url).ok().map(|url| Self::of(&url))
    }

    /// The spellings a crawler URL naming the same page as `url` can be stored under: `url`
    /// and its crawler normalization, each on http and https, with and without `www.`.
    pub(super) fn spellings(url: &Url) -> Vec<String> {
        let mut spellings = Vec::new();
        for base in [url.clone(), CrawledUrl::new(url.clone()).as_url().clone()] {
            let Some(host) = base.host_str().map(canonical_crawler_domain) else {
                continue;
            };
            for scheme in ["https", "http"] {
                for host in [host.clone(), format!("www.{host}")] {
                    let mut spelling = base.clone();
                    if spelling.set_scheme(scheme).is_ok() && spelling.set_host(Some(&host)).is_ok()
                    {
                        spellings.push(spelling.to_string());
                    }
                }
            }
        }
        spellings.sort_unstable();
        spellings.dedup();
        spellings
    }
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
    fn should_identify_same_review_page_when_url_differs_only_in_spelling() {
        let stored = ReviewPageIdentity::of_stored("https://www.example.com/products/1");

        for raw in [
            "https://www.example.com/products/1/",
            "https://example.com/products/1",
            "http://www.example.com/products/1",
        ] {
            assert_eq!(stored, Some(ReviewPageIdentity::of(&url(raw))), "{raw}");
        }
        for raw in [
            "https://www.example.com/products/2",
            "https://www.example.com/products/1?variant=sold",
        ] {
            assert_ne!(stored, Some(ReviewPageIdentity::of(&url(raw))), "{raw}");
        }
    }

    #[test]
    fn should_spell_a_page_url_as_the_crawler_may_have_stored_it() {
        let page = url("https://example.com/products/1/");

        let spellings = ReviewPageIdentity::spellings(&page);

        for stored in [
            "https://example.com/products/1",
            "https://example.com/products/1/",
            "https://www.example.com/products/1",
            "http://example.com/products/1",
        ] {
            assert!(
                spellings.iter().any(|spelling| spelling == stored),
                "{stored}"
            );
        }
        let identity = ReviewPageIdentity::of(&page);
        assert!(
            spellings
                .iter()
                .all(|spelling| ReviewPageIdentity::of_stored(spelling).as_ref() == Some(&identity))
        );
    }

    #[test]
    fn should_drop_fragment_when_reviewer_page_url_is_normalized() {
        let normalized = reviewer_page_url(&url("https://example.com/products/1#gallery"));

        assert_eq!(normalized.as_str(), "https://example.com/products/1");
    }
}
