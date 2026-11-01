use crate::network::policy::canonical_crawler_domain;
use crate::spider::utils::url::CrawledUrl;
use url::Url;

/// The URL a reviewer page is checked and stored under: the fragment never reaches the shop,
/// so `…/1` and `…/1#gallery` are the same page.
pub(super) fn reviewer_page_url(url: &Url) -> Url {
    let mut url = url.clone();
    url.set_fragment(None);
    url
}

/// Which shop page a review page URL names: the crawler's [page](CrawledUrl::page_spelling) of
/// the URL (with or without its trailing slash, without fragment, default port and similar
/// spellings) on the crawler's ownership host, which treats `www.` and the bare host as one
/// domain, regardless of http or https.
#[derive(Debug, PartialEq, Eq, Hash)]
pub(super) struct ReviewPageIdentity {
    host: Option<String>,
    path: String,
    query: Option<String>,
}

impl ReviewPageIdentity {
    pub(super) fn of(url: &Url) -> Self {
        let page = CrawledUrl::new(url.clone()).page_spelling();
        Self {
            host: page.host_str().map(canonical_crawler_domain),
            path: page.path().to_owned(),
            query: page.query().map(str::to_owned),
        }
    }

    pub(super) fn of_stored(stored_url: &str) -> Option<Self> {
        Url::parse(stored_url).ok().map(|url| Self::of(&url))
    }

    /// The spellings a crawler URL naming the same page as `url` can be stored under: `url`, its
    /// crawler spelling and that spelling with and without its trailing slash, each on http and
    /// https, with and without `www.`.
    pub(super) fn spellings(url: &Url) -> Vec<String> {
        let crawled = CrawledUrl::new(url.clone());
        let page = CrawledUrl::new(crawled.page_spelling());
        let bases = [url.clone(), crawled.as_url().clone(), page.as_url().clone()]
            .into_iter()
            .chain(page.slashed_twin());
        let mut spellings = Vec::new();
        for base in bases {
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

    /// The stored URL `stored_url` first, then the other [spellings](Self::spellings) of its
    /// page: what a review page and a crawler URL naming one page may each be stored under. A
    /// value that is not a URL is only itself.
    pub(super) fn stored_spellings(stored_url: &str) -> Vec<String> {
        let mut spellings = vec![stored_url.to_owned()];
        if let Ok(url) = Url::parse(stored_url) {
            spellings.extend(
                Self::spellings(&url)
                    .into_iter()
                    .filter(|spelling| spelling != stored_url),
            );
        }
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

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
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
            "https://example.com/products/1//",
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

    /// A crawler URL stored before the crawler kept the trailing slash lacks it, and one the
    /// spider stored as fetched may carry it where the review page does not.
    #[test]
    fn should_spell_a_page_url_as_the_crawler_may_have_stored_it() {
        for page in [
            url("https://example.com/products/1/"),
            url("https://example.com/products/1"),
            url("https://example.com/products/1#gallery"),
        ] {
            let spellings = ReviewPageIdentity::spellings(&page);

            for stored in [
                "https://example.com/products/1",
                "https://example.com/products/1/",
                "https://www.example.com/products/1",
                "https://www.example.com/products/1/",
                "http://example.com/products/1",
            ] {
                assert!(
                    spellings.iter().any(|spelling| spelling == stored),
                    "{page}: {stored}"
                );
            }
            let identity = ReviewPageIdentity::of(&page);
            assert!(
                spellings
                    .iter()
                    .all(|spelling| ReviewPageIdentity::of_stored(spelling).as_ref()
                        == Some(&identity)),
                "{page}"
            );
        }
    }

    /// The rejected-page lookup and its revocation match a crawler URL and a review page with the
    /// same notion of one page as the console: trailing slash, `http`/`https` and `www.`.
    #[test]
    fn should_spell_a_stored_url_as_every_spelling_of_its_page() {
        for (stored, same_page) in [
            (
                "https://example.com/products/1",
                [
                    "https://example.com/products/1/",
                    "http://example.com/products/1",
                    "https://www.example.com/products/1",
                    "http://www.example.com/products/1/",
                ],
            ),
            (
                "http://www.example.com/products/1/?q=1",
                [
                    "https://example.com/products/1?q=1",
                    "https://example.com/products/1/?q=1",
                    "http://example.com/products/1?q=1",
                    "https://www.example.com/products/1/?q=1",
                ],
            ),
        ] {
            let spellings = ReviewPageIdentity::stored_spellings(stored);

            assert_eq!(spellings.first().map(String::as_str), Some(stored));
            for spelling in same_page {
                assert!(
                    spellings.iter().any(|s| s == spelling),
                    "{stored}: {spelling}"
                );
            }
            assert!(
                !spellings.iter().any(|s| s.contains("/products/2")),
                "{stored}: {spellings:?}"
            );
        }
        assert_eq!(
            ReviewPageIdentity::stored_spellings("https://example.com/"),
            vec![
                "https://example.com/",
                "http://example.com/",
                "http://www.example.com/",
                "https://www.example.com/",
            ]
        );
        assert_eq!(
            ReviewPageIdentity::stored_spellings("not a url"),
            vec!["not a url"]
        );
    }

    #[test]
    fn should_drop_fragment_when_reviewer_page_url_is_normalized() {
        let normalized = reviewer_page_url(&url("https://example.com/products/1#gallery"));

        assert_eq!(normalized.as_str(), "https://example.com/products/1");
    }
}
