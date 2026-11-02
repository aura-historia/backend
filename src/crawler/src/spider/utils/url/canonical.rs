//! The canonical URL of a product page the shop serves under several URLs
//! (`docs/crawler/networking.md#canonical-product-urls`).
//!
//! A page names its canonical URL with `<link rel="canonical">`, a language copy names the page
//! it translates among its `hreflang` alternates, and a known shop system serves products under
//! URLs whose canonical form is fixed. Each rule only ever proposes a URL on the page's own
//! origin that the product pattern takes, so a shop's misconfigured canonical link cannot move a
//! product onto another host or onto a category or home page.

use regex::Regex;
use std::sync::LazyLock;

use super::CrawledUrl;

/// A shop system whose product URLs the crawler knows how to canonicalize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopPlatform {
    Shopify,
}

impl ShopPlatform {
    pub const ALL: [ShopPlatform; 1] = [ShopPlatform::Shopify];
}

/// What a fetched page says about its own identity. Its links are already resolved, on the
/// page's own origin, and fetchable by the crawl that found the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageIdentityEvidence {
    /// The page's `<link rel="canonical">`.
    pub canonical_link: Option<CrawledUrl>,
    /// The targets of the page's `<link rel="alternate" hreflang>` links.
    pub language_alternates: Vec<CrawledUrl>,
    /// The shop system the page was served by, when it carries that system's markers.
    pub platform: Option<ShopPlatform>,
    /// The product URL each shop system serves the page under instead
    /// ([`platform_product_url`]), when the crawl may fetch it: robots.txt allows it and the
    /// crawler does not refuse it.
    pub platform_product_urls: Vec<(ShopPlatform, CrawledUrl)>,
}

impl PageIdentityEvidence {
    fn platform_product_url(&self, platform: ShopPlatform) -> Option<CrawledUrl> {
        self.platform_product_urls
            .iter()
            .find(|(candidate, _)| *candidate == platform)
            .map(|(_, url)| url.clone())
    }
}

/// The URL a product page is stored under instead of `page`, or `None` when the page is its own
/// canonical URL or nothing proves another. The rules apply in this order:
///
/// 1. the page's canonical link,
/// 2. the alternate whose URL is `page` without a leading language segment (`/en/x` for `/x`),
/// 3. the platform's product URL (Shopify serves `/collections/<c>/products/<p>` as
///    `/products/<p>`), as the evidence holds it once the crawl found it may fetch it.
///
/// A rule proposes a URL only when `product_pattern` takes both `page` and the proposal, the
/// proposal is another page on `page`'s origin and the crawler would fetch it; otherwise the
/// next rule is asked.
pub fn canonical_product_url(
    page: &CrawledUrl,
    evidence: &PageIdentityEvidence,
    platform: Option<ShopPlatform>,
    product_pattern: &Regex,
) -> Option<CrawledUrl> {
    if !page.matches_pattern(product_pattern) {
        return None;
    }
    let language_original = evidence
        .language_alternates
        .iter()
        .find(|alternate| is_language_copy_of(page, alternate))
        .cloned();
    let platform_url = platform.and_then(|platform| evidence.platform_product_url(platform));
    [
        evidence.canonical_link.clone(),
        language_original,
        platform_url,
    ]
    .into_iter()
    .flatten()
    .find(|candidate| {
        candidate.as_url().origin() == page.as_url().origin()
            && !candidate.is_same_page(page)
            && !candidate.is_blacklisted()
            && candidate.matches_pattern(product_pattern)
    })
}

/// A language segment as shops put it before a path: an ISO 639 code with an optional region or
/// script (`en`, `de-at`, `pt_BR`, `zh-hans`).
static LANGUAGE_SEGMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z]{2,3}(?:[-_][A-Za-z0-9]{2,4})?$").expect("language segment regex")
});

/// Whether a path segment is a language segment as shops put it before a path.
pub(crate) fn is_language_segment(segment: &str) -> bool {
    LANGUAGE_SEGMENT.is_match(segment)
}

/// The page `page` names without the language segment before its path, in its page spelling:
/// `/x` for `/en/x`. `None` when the path starts with no language segment or has nothing after
/// it.
pub(crate) fn without_language_segment(page: &CrawledUrl) -> Option<url::Url> {
    let mut url = page.page_spelling();
    let (language, rest) = url.path().trim_start_matches('/').split_once('/')?;
    let rest = rest.trim_end_matches('/');
    if !is_language_segment(language) || rest.is_empty() {
        return None;
    }
    let path = format!("/{rest}");
    url.set_path(&path);
    Some(url)
}

/// Whether `page` is `original` with a language segment before its path: `/en/x` is a copy of
/// `/x` on the same origin with the same query.
fn is_language_copy_of(page: &CrawledUrl, original: &CrawledUrl) -> bool {
    without_language_segment(page).is_some_and(|unprefixed| unprefixed == original.page_spelling())
}

/// Shopify serves every product under `/products/<handle>` and also under each collection that
/// lists it, optionally behind a market's language segment.
static SHOPIFY_COLLECTION_PRODUCT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<language>/[A-Za-z]{2}(?:-[A-Za-z]{2})?)?/collections/[^/]+/products/(?P<handle>[^/]+)/?$",
    )
    .expect("Shopify collection product regex")
});

/// The URL `platform` serves the product page `page` under instead, when `page` is one of its
/// alternate product URLs.
pub fn platform_product_url(page: &CrawledUrl, platform: ShopPlatform) -> Option<CrawledUrl> {
    match platform {
        ShopPlatform::Shopify => {
            let captures = SHOPIFY_COLLECTION_PRODUCT.captures(page.as_url().path())?;
            let language = captures.name("language").map_or("", |m| m.as_str());
            let mut url = page.as_url().clone();
            url.set_path(&format!("{language}/products/{}", &captures["handle"]));
            Some(CrawledUrl::new(url))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crawled(url: &str) -> CrawledUrl {
        CrawledUrl::try_from(url).unwrap()
    }

    /// The unanchored pattern URL-pattern inference produces for a Shopify shop: it takes the
    /// product under every collection as well.
    fn shopify_pattern() -> Regex {
        Regex::new(r"/products/[^/]+$").unwrap()
    }

    fn canonical(
        page: &str,
        evidence: &PageIdentityEvidence,
        platform: Option<ShopPlatform>,
        pattern: &Regex,
    ) -> Option<String> {
        canonical_product_url(&crawled(page), evidence, platform, pattern)
            .map(|url| url.to_string())
    }

    fn with_canonical_link(url: &str) -> PageIdentityEvidence {
        PageIdentityEvidence {
            canonical_link: Some(crawled(url)),
            ..PageIdentityEvidence::default()
        }
    }

    fn with_alternates(urls: &[&str]) -> PageIdentityEvidence {
        PageIdentityEvidence {
            language_alternates: urls.iter().map(|url| crawled(url)).collect(),
            ..PageIdentityEvidence::default()
        }
    }

    mod canonical_link {
        use super::*;

        #[test]
        fn should_store_a_shopify_collection_url_under_its_canonical_product_url() {
            assert_eq!(
                canonical(
                    "https://frenchmetro.com/collections/all/products/22bs062-ladle",
                    &with_canonical_link("https://frenchmetro.com/products/22bs062-ladle"),
                    None,
                    &shopify_pattern(),
                ),
                Some("https://frenchmetro.com/products/22bs062-ladle".to_owned())
            );
        }

        #[test]
        fn should_keep_the_page_when_its_canonical_link_names_the_page_itself() {
            for canonical_link in [
                "https://shop.example/products/chair",
                "https://shop.example/products/chair/",
            ] {
                assert_eq!(
                    canonical(
                        "https://shop.example/products/chair",
                        &with_canonical_link(canonical_link),
                        None,
                        &shopify_pattern(),
                    ),
                    None,
                    "{canonical_link}"
                );
            }
        }

        #[test]
        fn should_keep_the_page_when_its_canonical_link_names_a_non_product_page() {
            for canonical_link in [
                "https://shop.example/",
                "https://shop.example/collections/chairs",
            ] {
                assert_eq!(
                    canonical(
                        "https://shop.example/products/chair",
                        &with_canonical_link(canonical_link),
                        None,
                        &shopify_pattern(),
                    ),
                    None,
                    "{canonical_link}"
                );
            }
        }

        #[test]
        fn should_keep_the_page_when_its_canonical_link_is_on_another_origin() {
            for canonical_link in [
                "https://other.example/products/chair",
                "https://www.shop.example/products/chair",
                "http://shop.example/products/chair",
            ] {
                assert_eq!(
                    canonical(
                        "https://shop.example/collections/all/products/chair",
                        &with_canonical_link(canonical_link),
                        None,
                        &shopify_pattern(),
                    ),
                    None,
                    "{canonical_link}"
                );
            }
        }

        #[test]
        fn should_keep_the_page_when_its_canonical_link_is_a_page_the_crawler_never_fetches() {
            let pattern = Regex::new(r"/(products|cart)/").unwrap();
            assert_eq!(
                canonical(
                    "https://shop.example/products/chair/",
                    &with_canonical_link("https://shop.example/cart/"),
                    None,
                    &pattern,
                ),
                None
            );
        }

        #[test]
        fn should_not_canonicalize_a_page_that_is_no_product_url() {
            assert_eq!(
                canonical(
                    "https://shop.example/collections/chairs",
                    &with_canonical_link("https://shop.example/products/chair"),
                    None,
                    &shopify_pattern(),
                ),
                None
            );
        }
    }

    mod language_copy {
        use super::*;

        const DE_PAGE: &str =
            "https://www.antik-heymann.de/antiquitaeten/tische/biedermeier-tisch-1022.html";
        const EN_PAGE: &str =
            "https://www.antik-heymann.de/en/antiquitaeten/tische/biedermeier-tisch-1022.html";

        fn antique_pattern() -> Regex {
            Regex::new(r"/antiquitaeten/[^/]+/[^/]+\.html$").unwrap()
        }

        /// antik-heymann.de serves each product under `/en/` too; the English page names itself
        /// canonical and the German page as its `de-DE` and `x-default` alternate.
        #[test]
        fn should_store_a_language_prefixed_copy_under_the_page_it_translates() {
            let evidence = PageIdentityEvidence {
                canonical_link: Some(crawled(EN_PAGE)),
                language_alternates: vec![crawled(DE_PAGE), crawled(EN_PAGE), crawled(DE_PAGE)],
                ..PageIdentityEvidence::default()
            };

            assert_eq!(
                canonical(EN_PAGE, &evidence, None, &antique_pattern()),
                Some(DE_PAGE.to_owned())
            );
        }

        #[test]
        fn should_keep_the_original_language_page_as_it_is() {
            assert_eq!(
                canonical(
                    DE_PAGE,
                    &with_alternates(&[DE_PAGE, EN_PAGE]),
                    None,
                    &antique_pattern()
                ),
                None
            );
        }

        #[test]
        fn should_keep_a_language_page_whose_alternates_are_all_prefixed() {
            let fr_page = DE_PAGE.replace(".de/", ".de/fr/");
            let de_page = DE_PAGE.replace(".de/", ".de/de/");
            assert_eq!(
                canonical(
                    &fr_page,
                    &with_alternates(&[&de_page, &fr_page]),
                    None,
                    &antique_pattern()
                ),
                None
            );
        }

        #[test]
        fn should_keep_a_page_whose_first_segment_is_no_language() {
            let pattern = Regex::new(r"/item/").unwrap();
            assert_eq!(
                canonical(
                    "https://shop.example/antiques/item/1",
                    &with_alternates(&["https://shop.example/item/1"]),
                    None,
                    &pattern
                ),
                None
            );
        }
    }

    mod platform_rule {
        use super::*;

        /// `evidence` of `page` on a crawl that may fetch the page's Shopify product URL.
        fn fetchable_shopify_url(
            page: &str,
            evidence: PageIdentityEvidence,
        ) -> PageIdentityEvidence {
            PageIdentityEvidence {
                platform_product_urls: platform_product_url(&crawled(page), ShopPlatform::Shopify)
                    .map(|url| (ShopPlatform::Shopify, url))
                    .into_iter()
                    .collect(),
                ..evidence
            }
        }

        #[test]
        fn should_store_a_shopify_collection_product_url_under_its_product_url() {
            let pattern = Regex::new(r"/products/[^/]+/?$").unwrap();
            for (page, expected) in [
                (
                    "https://kernow.example/collections/chairs/products/oak-chair",
                    "https://kernow.example/products/oak-chair",
                ),
                (
                    "https://kernow.example/collections/chairs/products/oak-chair/",
                    "https://kernow.example/products/oak-chair",
                ),
                (
                    "https://kernow.example/en-gb/collections/chairs/products/oak-chair?variant=1",
                    "https://kernow.example/en-gb/products/oak-chair",
                ),
            ] {
                assert_eq!(
                    canonical(
                        page,
                        &fetchable_shopify_url(page, PageIdentityEvidence::default()),
                        Some(ShopPlatform::Shopify),
                        &pattern,
                    ),
                    Some(expected.to_owned()),
                    "{page}"
                );
            }
        }

        #[test]
        fn should_not_apply_the_shopify_rule_to_a_shop_not_known_to_run_on_shopify() {
            let page = "https://shop.example/collections/chairs/products/oak-chair";
            assert_eq!(
                canonical(
                    page,
                    &fetchable_shopify_url(page, PageIdentityEvidence::default()),
                    None,
                    &shopify_pattern(),
                ),
                None
            );
        }

        /// The crawl leaves the Shopify product URL out of the evidence when robots.txt
        /// disallows it.
        #[test]
        fn should_keep_the_fetched_url_when_the_crawl_may_not_fetch_the_shopify_product_url() {
            assert_eq!(
                canonical(
                    "https://kernow.example/collections/chairs/products/oak-chair",
                    &PageIdentityEvidence::default(),
                    Some(ShopPlatform::Shopify),
                    &shopify_pattern(),
                ),
                None
            );
        }

        #[test]
        fn should_prefer_the_canonical_link_over_the_platform_rule() {
            assert_eq!(
                canonical(
                    "https://kernow.example/collections/chairs/products/oak-chair",
                    &fetchable_shopify_url(
                        "https://kernow.example/collections/chairs/products/oak-chair",
                        with_canonical_link("https://kernow.example/products/oak-chair-1"),
                    ),
                    Some(ShopPlatform::Shopify),
                    &shopify_pattern(),
                ),
                Some("https://kernow.example/products/oak-chair-1".to_owned())
            );
        }

        #[test]
        fn should_fall_back_to_the_platform_rule_when_the_canonical_link_is_refused() {
            assert_eq!(
                canonical(
                    "https://kernow.example/collections/chairs/products/oak-chair",
                    &fetchable_shopify_url(
                        "https://kernow.example/collections/chairs/products/oak-chair",
                        with_canonical_link("https://kernow.example/collections/chairs"),
                    ),
                    Some(ShopPlatform::Shopify),
                    &shopify_pattern(),
                ),
                Some("https://kernow.example/products/oak-chair".to_owned())
            );
        }
    }
}
