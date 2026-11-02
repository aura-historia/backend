//! The pages an incremental crawl already stores and does not fetch again
//! (`docs/crawler/scheduling-and-retries.md#discovery-coverage`).

use spider::CaseInsensitiveString;
use std::collections::HashMap;
use url::Url;

use super::frontier::CrawlSeeds;
use crate::spider::utils::url::CrawledUrl;

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

    /// The known page `url` names, in its page spelling, and what it is to the crawl.
    pub(super) fn page_of(&self, url: &Url) -> Option<(Url, KnownPage)> {
        let stored = CrawledUrl::new(url.clone());
        self.spellings
            .get(&CaseInsensitiveString::from(stored.as_url().as_str()))
            .map(|page| (stored.page_spelling(), *page))
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
}
