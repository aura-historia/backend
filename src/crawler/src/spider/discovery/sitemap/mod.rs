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

use url::Url;

use crate::spider::classification::product_url_pattern::ProductUrlPattern;
use crate::spider::discovery::frontier::SeedAdmission;
use crate::spider::discovery::known_pages::KnownPages;
use crate::spider::utils::url::CrawledUrl;
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
pub(in crate::spider::discovery) fn register(
    pages: &[Url],
    admission: &SeedAdmission<'_>,
    product_pattern: Option<&ProductUrlPattern>,
    known: &KnownPages,
    diagnostics: &mut SitemapDiagnostics,
) -> SitemapRegistration {
    let mut registration = SitemapRegistration::default();
    for page in pages {
        let Some(stored) = admission.admit(page) else {
            diagnostics.urls_refused += 1;
            continue;
        };
        if !product_pattern.is_some_and(|pattern| stored.matches_pattern(pattern)) {
            registration.seeds.push(stored.as_url().clone());
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
