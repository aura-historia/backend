//! The pages an incremental crawl already stores and does not fetch again
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).

use spider::CaseInsensitiveString;
use spider::fetch_engine::{EngineError, EngineRequest, EngineResponse, HttpFetchEngine};
use spider::reqwest::StatusCode;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use url::Url;

use super::frontier::CrawlSeeds;
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::{ShopPlatform, platform_product_url};

/// What a known page is to the crawl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KnownPage {
    /// A product page the domain stores, which the crawl reports as found linked.
    Product,
    /// A page retired as a duplicate of another, which is no product page of its own.
    Retired,
}

/// The pages an incremental crawl knows, under every spelling Spider may meet them by: a page
/// linked with and without its trailing slash is one page
/// (`docs/crawler/networking.md#stored-urls`). Never the crawl root, which stays crawlable.
#[derive(Debug, Default)]
pub(super) struct KnownPages {
    spellings: HashMap<CaseInsensitiveString, KnownPage>,
}

impl KnownPages {
    /// The known product pages and retired pages of `seeds`, without `root`. A page both stored
    /// as a product and retired counts as a product.
    pub(super) fn new(root: &Url, seeds: &CrawlSeeds) -> Self {
        let root = CaseInsensitiveString::from(root.as_str());
        let spellings_of = |urls: &[Url], page: KnownPage| {
            urls.iter()
                .flat_map(|url| CrawledUrl::new(url.clone()).page_spellings())
                .map(|url| (CaseInsensitiveString::from(url.as_str()), page))
                .collect::<Vec<_>>()
        };
        let spellings = spellings_of(&seeds.retired, KnownPage::Retired)
            .into_iter()
            .chain(spellings_of(&seeds.known, KnownPage::Product))
            .filter(|(spelling, _)| *spelling != root)
            .collect();
        Self { spellings }
    }

    /// Every spelling of every known page, as Spider compares links.
    pub(super) fn spellings(&self) -> impl Iterator<Item = &CaseInsensitiveString> {
        self.spellings.keys()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.spellings.is_empty()
    }

    /// The known page `url` names, in its page spelling, and what it is to the crawl: the page
    /// stored under `url` in the spelling the crawler stores
    /// (`docs/crawler/networking.md#stored-urls`), or else the stored product page a shop system
    /// serves `url` as. The crawl does not wait to see the shop's system: a stored
    /// `/products/<handle>` and a link of Shopify's collection shape to the same handle are
    /// evidence enough to leave the link to the next full crawl.
    pub(super) fn page_of(&self, url: &Url) -> Option<(Url, KnownPage)> {
        let stored = CrawledUrl::new(url.clone());
        self.stored_page(&stored).or_else(|| {
            ShopPlatform::ALL
                .into_iter()
                .find_map(|platform| self.stored_page(&platform_product_url(&stored, platform)?))
        })
    }

    fn stored_page(&self, url: &CrawledUrl) -> Option<(Url, KnownPage)> {
        self.spellings
            .get(&CaseInsensitiveString::from(url.as_url().as_str()))
            .map(|page| (url.page_spelling(), *page))
    }
}

/// Answers Spider's request for a link whose page the crawl already knows without a request
/// (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).
///
/// Spider checks its visited set, where the known spellings are marked, against a link as the
/// page writes it, and hands the link decoder the link only when it fetches it. A known page
/// linked with session or tracking parameters, or under a Shopify collection, passes that check.
/// A fetch engine is the one veto Spider consults with the URL it requests: this one serves
/// those links an empty `304 Not Modified`, which Spider neither retries nor reads links from,
/// and the relay drops the answer by its URL.
#[derive(Debug, Default)]
pub(super) struct KnownPageAnswers {
    known: KnownPages,
    answered: Mutex<HashSet<String>>,
}

impl KnownPageAnswers {
    pub(super) fn new(known: KnownPages) -> Self {
        Self {
            known,
            answered: Mutex::default(),
        }
    }

    pub(super) fn known(&self) -> &KnownPages {
        &self.known
    }

    /// How many links this answered instead of the shop, each link once.
    pub(super) fn answered_links(&self) -> usize {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether Spider delivers `page_url` with the answer given here instead of the shop's.
    pub(super) fn answered(&self, page_url: &str) -> bool {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(page_url)
    }
}

#[async_trait::async_trait]
impl HttpFetchEngine for KnownPageAnswers {
    async fn fetch(&self, request: EngineRequest<'_>) -> Result<EngineResponse, EngineError> {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.url.to_owned());
        Ok(EngineResponse {
            status_code: StatusCode::NOT_MODIFIED,
            ..EngineResponse::default()
        })
    }

    /// Whether the link Spider is about to request names a known page; Spider requests every
    /// other link itself.
    fn should_fetch(&self, url: &str) -> bool {
        Url::parse(url).is_ok_and(|url| self.known.page_of(&url).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
    }

    fn known(products: &[&str], retired: &[&str]) -> KnownPages {
        KnownPages::new(
            &url("/"),
            &CrawlSeeds {
                known: products.iter().map(|path| url(path)).collect(),
                retired: retired.iter().map(|path| url(path)).collect(),
                ..CrawlSeeds::default()
            },
        )
    }

    #[test]
    fn should_know_a_page_under_both_trailing_slash_spellings() {
        let known = known(&["/product/1/"], &[]);

        for spelling in ["/product/1", "/product/1/"] {
            assert_eq!(
                known.page_of(&url(spelling)),
                Some((url("/product/1"), KnownPage::Product)),
                "{spelling}"
            );
        }
    }

    #[test]
    fn should_tell_retired_pages_from_product_pages_and_never_know_the_root() {
        let known = known(&["/", "/product/1"], &["/old/1", "/product/1"]);

        assert_eq!(
            known.page_of(&url("/old/1")),
            Some((url("/old/1"), KnownPage::Retired))
        );
        assert_eq!(
            known.page_of(&url("/product/1")),
            Some((url("/product/1"), KnownPage::Product))
        );
        assert_eq!(known.page_of(&url("/")), None);
        assert_eq!(known.page_of(&url("/product/2")), None);
    }

    #[test]
    fn should_know_a_page_linked_with_parameters_that_do_not_identify_it() {
        let known = known(&["/product/1", "/item.php?id=7"], &[]);

        for (link, page) in [
            ("/product/1?utm_source=newsletter", "/product/1"),
            ("/product/1/?sessionid=abc#top", "/product/1"),
            ("/item.php?id=7&PHPSESSID=abc", "/item.php?id=7"),
        ] {
            assert_eq!(
                known.page_of(&url(link)),
                Some((url(page), KnownPage::Product)),
                "{link}"
            );
        }
        assert_eq!(known.page_of(&url("/item.php?id=8&PHPSESSID=abc")), None);
    }

    #[test]
    fn should_know_a_product_linked_under_a_shopify_collection() {
        let known = known(&["/products/oak-chair", "/en-gb/products/oak-chair"], &[]);

        for (link, page) in [
            (
                "/collections/chairs/products/oak-chair?variant=1",
                "/products/oak-chair",
            ),
            (
                "/en-gb/collections/sale/products/oak-chair/",
                "/en-gb/products/oak-chair",
            ),
        ] {
            assert_eq!(
                known.page_of(&url(link)),
                Some((url(page), KnownPage::Product)),
                "{link}"
            );
        }
        assert_eq!(
            known.page_of(&url("/collections/chairs/products/pine-chair")),
            None
        );
    }

    #[tokio::test]
    async fn should_answer_only_links_to_known_pages_and_remember_them() {
        let answers = KnownPageAnswers::new(known(&["/product/1"], &[]));
        let link = "https://shop.example/product/1?utm_source=x";

        assert!(answers.should_fetch(link));
        assert!(!answers.should_fetch("https://shop.example/product/2"));
        assert!(!answers.answered(link));

        let configuration = spider::configuration::Configuration::default();
        let answer = answers
            .fetch(EngineRequest {
                url: link,
                method: spider::fetch_engine::EngineMethod::Get,
                configuration: &configuration,
                only_html: true,
                attempt: 0,
                conditional_headers: None,
            })
            .await
            .unwrap();

        assert_eq!(answer.status_code, StatusCode::NOT_MODIFIED);
        assert!(answer.body.is_empty());
        assert!(answers.answered(link));
        assert_eq!(answers.answered_links(), 1);
    }
}
