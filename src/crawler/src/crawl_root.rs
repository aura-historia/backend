//! Where a ListingSource is crawled, derived from the presentation URL the admin API gives it
//! (`docs/crawler/README.md#listingsource-scope`).

use crate::network::policy::canonical_crawl_host;
use listing_source_core::Domain;
use std::net::IpAddr;
use url::Url;

/// The page a ListingSource's crawl starts at and the host the crawl stays on.
///
/// The root is the presentation URL on `https`, without credentials, query or fragment: the
/// crawler has no plain-HTTP fallback, and the path names the start page, not a scope. The host is
/// the root's host without one leading `www.`, so a shop's bare and `www.` hosts are one scope and
/// own the same URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlRoot {
    url: Url,
    host: String,
}

/// Why a presentation URL names no crawl root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CrawlRootRejection {
    #[error("the presentation URL does not parse")]
    Unparsable,
    #[error("the presentation URL is neither http nor https")]
    NotHttp,
    #[error("the presentation URL carries credentials")]
    Credentials,
    #[error("the presentation URL names a port")]
    ExplicitPort,
    #[error("the presentation URL's host is no domain name")]
    NoDomainHost,
    #[error("the presentation URL's host repeats a leading www.")]
    RepeatedWww,
}

/// A stored crawl root that is not the root its own URL derives.
#[derive(Debug, thiserror::Error)]
#[error("persisted crawl root {url:?} with host {host:?} is invalid")]
pub struct InvalidPersistedCrawlRoot {
    url: String,
    host: String,
}

impl CrawlRoot {
    /// The crawl root of a shop presented at `presentation_url`.
    pub fn from_presentation_url(presentation_url: &str) -> Result<Self, CrawlRootRejection> {
        let presented =
            Url::parse(presentation_url.trim()).map_err(|_| CrawlRootRejection::Unparsable)?;
        if !matches!(presented.scheme(), "http" | "https") {
            return Err(CrawlRootRejection::NotHttp);
        }
        if !presented.username().is_empty() || presented.password().is_some() {
            return Err(CrawlRootRejection::Credentials);
        }
        if presented.port().is_some() {
            return Err(CrawlRootRejection::ExplicitPort);
        }
        let root_host = presented
            .host_str()
            .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
            .filter(|host| host.parse::<IpAddr>().is_err())
            .and_then(|host| Domain::try_from(host).ok())
            .ok_or(CrawlRootRejection::NoDomainHost)?;
        if root_host.as_str().starts_with("www.www.") {
            return Err(CrawlRootRejection::RepeatedWww);
        }
        let mut url = Url::parse(&format!("https://{}", root_host.as_str()))
            .map_err(|_| CrawlRootRejection::NoDomainHost)?;
        url.set_path(presented.path());
        Ok(Self {
            host: canonical_crawl_host(root_host.as_str()),
            url,
        })
    }

    /// The crawl root `listing_sources` stores, which must be the one its URL derives.
    pub fn from_persisted(url: &str, host: &str) -> Result<Self, InvalidPersistedCrawlRoot> {
        Self::from_presentation_url(url)
            .ok()
            .filter(|root| root.url.as_str() == url && root.host == host)
            .ok_or_else(|| InvalidPersistedCrawlRoot {
                url: url.to_owned(),
                host: host.to_owned(),
            })
    }

    /// The `https` page the crawl starts at.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The root's host without one leading `www.`.
    pub fn host(&self) -> &str {
        &self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::bare_host("https://shop.example", "https://shop.example/", "shop.example")]
    #[case::www_host(
        "https://www.shop.example/",
        "https://www.shop.example/",
        "shop.example"
    )]
    #[case::http_is_crawled_on_https(
        "http://shop.example/",
        "https://shop.example/",
        "shop.example"
    )]
    #[case::path_is_the_start_page(
        "https://shop.example/en/antiques/?utm_source=x#top",
        "https://shop.example/en/antiques/",
        "shop.example"
    )]
    #[case::host_is_lowercased_without_trailing_dot(
        "https://WWW.Shop.Example./Stock",
        "https://www.shop.example/Stock",
        "shop.example"
    )]
    #[case::surrounding_whitespace(
        " https://shop.example ",
        "https://shop.example/",
        "shop.example"
    )]
    fn should_derive_the_crawl_root_from_a_presentation_url(
        #[case] presentation_url: &str,
        #[case] root_url: &str,
        #[case] host: &str,
    ) {
        let root = CrawlRoot::from_presentation_url(presentation_url)
            .unwrap_or_else(|error| panic!("{presentation_url} must name a crawl root: {error}"));

        assert_eq!(root.url().as_str(), root_url);
        assert_eq!(root.host(), host);
    }

    #[rstest]
    #[case::not_a_url("shop.example", CrawlRootRejection::Unparsable)]
    #[case::other_scheme("ftp://shop.example/", CrawlRootRejection::NotHttp)]
    #[case::credentials("https://user:secret@shop.example/", CrawlRootRejection::Credentials)]
    #[case::port("https://shop.example:8443/", CrawlRootRejection::ExplicitPort)]
    #[case::ipv4("https://203.0.113.7/", CrawlRootRejection::NoDomainHost)]
    #[case::ipv6("https://[2001:db8::1]/", CrawlRootRejection::NoDomainHost)]
    #[case::single_label("https://localhost/", CrawlRootRejection::NoDomainHost)]
    #[case::repeated_www("https://www.www.shop.example/", CrawlRootRejection::RepeatedWww)]
    fn should_reject_a_presentation_url_that_names_no_crawlable_host(
        #[case] presentation_url: &str,
        #[case] rejection: CrawlRootRejection,
    ) {
        assert_eq!(
            CrawlRoot::from_presentation_url(presentation_url),
            Err(rejection)
        );
    }

    #[test]
    fn should_read_back_only_the_crawl_root_its_url_derives() {
        let stored = CrawlRoot::from_presentation_url("https://www.shop.example/en/")
            .unwrap_or_else(|error| panic!("valid crawl root: {error}"));

        assert_eq!(
            CrawlRoot::from_persisted(stored.url().as_str(), stored.host()).ok(),
            Some(stored)
        );
        for (url, host) in [
            ("https://www.shop.example/en/", "other.example"),
            ("http://www.shop.example/en/", "shop.example"),
            ("https://www.shop.example/en/?q=1", "shop.example"),
        ] {
            assert!(
                CrawlRoot::from_persisted(url, host).is_err(),
                "{url} with {host} must fail closed"
            );
        }
    }
}
