//! Sitemap-based product discovery (`docs/crawler/networking.md#sitemaps`).
//!
//! Sitemaps supplement a crawl, never replace it: a crawl reads the shop's sitemaps before Spider
//! starts, within the crawl's bounds, and the URLs they list pass the rules a found link passes.
//! With a product URL pattern, the URLs it takes are stored as product URLs without a fetch and
//! Spider does not fetch them; every other listed URL, and every listed URL of a ListingSource
//! without a pattern, becomes a crawl seed.

pub mod fetch;
pub mod locations;
pub mod parser;
pub mod reader;

use std::collections::HashSet;
use url::Url;

use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::discovery::frontier::SeedAdmission;
use crate::spider::discovery::known_pages::KnownPages;
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::without_language_segment;
use reader::SitemapDiagnostics;

/// What a crawl does with the pages its sitemaps list.
#[derive(Debug, Default)]
pub(in crate::spider::discovery) struct SitemapRegistration {
    /// Pages the product URL pattern takes and the ListingSource does not store yet: stored as
    /// product URLs without a fetch, and not fetched by the crawl.
    pub(in crate::spider::discovery) products: Vec<CrawledUrl>,
    /// Every other page, in the spelling Spider requests it, to seed the crawl with.
    pub(in crate::spider::discovery) seeds: Vec<Url>,
}

/// Splits the sitemap `pages` into product URLs and seeds. A page must pass the rules a found
/// link passes (`admission`: stored spelling, the root's host and scheme, the URL blacklist and
/// robots.txt); a product page the crawl already `known`s is neither registered nor seeded.
///
/// A product page listed both with and without a language segment before its path (`/x` and
/// `/en/x`, as multilingual Shopify sitemaps list every product) is registered once, without
/// the segment: the sitemap is the evidence the hreflang canonical rule reads from a page's head
/// for a fetched page (`docs/crawler/networking.md#canonical-product-urls`).
pub(in crate::spider::discovery) fn register(
    pages: &[Url],
    admission: &SeedAdmission<'_>,
    product_pattern: Option<&ProductUrlPattern>,
    known: &KnownPages,
    diagnostics: &mut SitemapDiagnostics,
) -> SitemapRegistration {
    let mut registration = SitemapRegistration::default();
    let mut taken = Vec::new();
    for page in pages {
        let Some(stored) = admission.admit(page) else {
            diagnostics.urls_refused += 1;
            continue;
        };
        if product_pattern.is_some_and(|pattern| stored.matches_pattern(pattern)) {
            taken.push(stored);
        } else {
            registration.seeds.push(stored.as_url().clone());
        }
    }
    let taken_pages: HashSet<Url> = taken.iter().map(CrawledUrl::page_spelling).collect();
    for stored in one_spelling_per_page(taken) {
        let language_copy = without_language_segment(&stored)
            .is_some_and(|page| taken_pages.contains(&page) || known.page_of(&page).is_some());
        if language_copy {
            diagnostics.products_language_copies += 1;
        } else if known.page_of(stored.as_url()).is_some() {
            diagnostics.products_known += 1;
        } else {
            registration.products.push(stored);
        }
    }
    diagnostics.products_registered = registration.products.len();
    diagnostics.seeds = registration.seeds.len();
    registration
}

/// Each page of `pages` once, in the order first listed, under its slashed spelling when the
/// sitemaps list both, as a crawl that meets both stores it
/// (`docs/crawler/networking.md#stored-urls`).
fn one_spelling_per_page(pages: Vec<CrawledUrl>) -> Vec<CrawledUrl> {
    let mut kept: Vec<CrawledUrl> = Vec::new();
    let mut index_of_page = std::collections::HashMap::new();
    for page in pages {
        match index_of_page.get(&page.page_spelling()) {
            Some(&index) => {
                if page.as_url().path().ends_with('/') {
                    kept[index] = page;
                }
            }
            None => {
                index_of_page.insert(page.page_spelling(), kept.len());
                kept.push(page);
            }
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::robots::DenyList;
    use crate::spider::discovery::frontier::{CrawlSeeds, FollowRules};
    use crate::spider::discovery::robots::CrawlRobots;
    use crate::spider::discovery::website_spider::configured_host_whitelist;
    use regex::{Regex, RegexSet};
    use std::sync::Arc;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://shop.example{path}")).unwrap()
    }

    fn register_listed(paths: &[&str]) -> (SitemapRegistration, SitemapDiagnostics) {
        register_listed_urls(paths.iter().map(|path| url(path)).collect())
    }

    fn register_listed_urls(pages: Vec<Url>) -> (SitemapRegistration, SitemapDiagnostics) {
        let root = url("/");
        let rules = FollowRules::new(
            root.clone(),
            Regex::new(&configured_host_whitelist(&root).unwrap()).unwrap(),
            Arc::new(RegexSet::empty()),
        );
        let robots = CrawlRobots {
            deny_list: DenyList::default(),
            crawl_delay: None,
            rules: None,
            policy: "unavailable_allow_all",
        };
        let admission = SeedAdmission {
            rules: &rules,
            robots: &robots,
        };
        let known = KnownPages::new(
            &root,
            &CrawlSeeds {
                known: vec![url("/products/stored")],
                ..CrawlSeeds::default()
            },
        );
        let pattern = ProductUrlPattern::parse("/products/[^/]+$").unwrap();
        let mut diagnostics = SitemapDiagnostics::default();
        let registration = register(&pages, &admission, Some(&pattern), &known, &mut diagnostics);
        (registration, diagnostics)
    }

    fn registered(registration: &SitemapRegistration) -> Vec<&str> {
        registration
            .products
            .iter()
            .map(|page| page.as_url().path())
            .collect()
    }

    #[test]
    fn should_register_a_product_listed_with_and_without_a_language_segment_once() {
        let (registration, diagnostics) = register_listed(&[
            "/products/chair",
            "/en/products/chair",
            "/de-at/products/chair",
            "/en/products/only-in-english",
            "/en/collections/chairs",
        ]);

        assert_eq!(
            registered(&registration),
            ["/products/chair", "/en/products/only-in-english"]
        );
        assert_eq!(diagnostics.products_language_copies, 2);
        assert_eq!(diagnostics.products_registered, 2);
        assert_eq!(diagnostics.seeds, 1);
    }

    #[test]
    fn should_register_a_product_listed_under_the_other_scheme_on_the_roots_scheme() {
        let listed = Url::parse("http://shop.example/products/legacy").unwrap();
        let (registration, _) = register_listed_urls(vec![listed]);

        assert_eq!(
            registration
                .products
                .iter()
                .map(|page| page.as_url().as_str())
                .collect::<Vec<_>>(),
            ["https://shop.example/products/legacy"]
        );
    }

    #[test]
    fn should_register_a_page_listed_in_several_spellings_once() {
        let (registration, diagnostics) = register_listed(&[
            "/products/twice",
            "/products/twice/",
            "/products/twice?utm_source=feed",
        ]);

        assert_eq!(registered(&registration), ["/products/twice/"]);
        assert_eq!(diagnostics.products_registered, 1);
    }

    #[test]
    fn should_not_register_a_language_copy_of_a_product_the_listing_source_stores() {
        let (registration, diagnostics) = register_listed(&["/en/products/stored"]);

        assert!(registration.products.is_empty());
        assert_eq!(diagnostics.products_language_copies, 1);
    }

    #[test]
    fn should_neither_register_nor_seed_a_product_the_listing_source_stores() {
        let (registration, diagnostics) =
            register_listed(&["/products/stored/", "/en/products/stored", "/products/new"]);

        assert_eq!(registered(&registration), ["/products/new"]);
        assert_eq!(diagnostics.products_known, 1);
        assert_eq!(diagnostics.products_language_copies, 1);
        assert!(registration.seeds.is_empty());
    }
}
