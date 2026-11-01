//! Reads the identity a fetched page declares in its `<head>`: its canonical link, its language
//! alternates and the shop system it was served by
//! (`docs/crawler/networking.md#canonical-product-urls`).

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::{PageIdentityEvidence, ShopPlatform};

/// How far into a page the `<head>` is looked for. Shopify heads run to about 60 KiB; a page
/// whose head ends later is read up to this bound only.
const MAX_HEAD_BYTES: usize = 512 * 1024;

const HEAD_END: &[u8] = b"</head>";

/// Markers Shopify puts into the head of every storefront page.
const SHOPIFY_MARKERS: [&str; 2] = ["Shopify.shop", "cdn.shopify.com"];

static IDENTITY_LINKS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("link[rel][href]").expect("identity link selector"));
static BASE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("base[href]").expect("base selector"));

/// The identity evidence of the page fetched as `page` whose response, after redirects, came
/// from `final_url` with `html` as its body. A link counts only when it resolves to an http(s)
/// URL on `page`'s origin that `admits` lets the crawl fetch. Conflicting canonical links, and
/// relative links on a page with a `<base>` element, are ignored instead of guessed at.
pub fn page_identity_evidence(
    page: &CrawledUrl,
    final_url: &Url,
    html: &[u8],
    admits: impl Fn(&CrawledUrl) -> bool,
) -> PageIdentityEvidence {
    let head = String::from_utf8_lossy(head_of(html));
    let platform = SHOPIFY_MARKERS
        .iter()
        .any(|marker| head.contains(marker))
        .then_some(ShopPlatform::Shopify);
    let lowercase_head = head.to_ascii_lowercase();
    if !lowercase_head.contains("canonical") && !lowercase_head.contains("hreflang") {
        return PageIdentityEvidence {
            platform,
            ..PageIdentityEvidence::default()
        };
    }

    let document = Html::parse_document(&head);
    let has_base = document.select(&BASE).next().is_some();
    let resolve = |href: &str| {
        let href = href.trim();
        if has_base && Url::parse(href).is_err() {
            return None;
        }
        let url = final_url.join(href).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.origin() != page.as_url().origin() {
            return None;
        }
        Some(CrawledUrl::new(url)).filter(|url| admits(url))
    };

    let mut canonical_links = Vec::new();
    let mut language_alternates = Vec::new();
    for link in document.select(&IDENTITY_LINKS) {
        let element = link.value();
        let rel = element.attr("rel").unwrap_or_default();
        let href = element.attr("href").unwrap_or_default();
        let has_rel = |name: &str| {
            rel.split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case(name))
        };
        if has_rel("canonical") {
            canonical_links.push(resolve(href));
        } else if has_rel("alternate")
            && element.attr("hreflang").is_some()
            && let Some(url) = resolve(href)
        {
            language_alternates.push(url);
        }
    }
    let canonical_link = match canonical_links.as_slice() {
        [Some(only)] => Some(only.clone()),
        [Some(first), rest @ ..] if rest.iter().all(|other| other.as_ref() == Some(first)) => {
            Some(first.clone())
        }
        _ => None,
    };
    PageIdentityEvidence {
        canonical_link,
        language_alternates,
        platform,
    }
}

/// The page up to the end of its `<head>`, at most [`MAX_HEAD_BYTES`].
fn head_of(html: &[u8]) -> &[u8] {
    let bounded = &html[..html.len().min(MAX_HEAD_BYTES)];
    bounded
        .windows(HEAD_END.len())
        .position(|window| window.eq_ignore_ascii_case(HEAD_END))
        .map_or(bounded, |end| &bounded[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://frenchmetro.com/collections/all/products/ladle";

    fn evidence_of(head: &str) -> PageIdentityEvidence {
        evidence_admitting(head, |_| true)
    }

    fn evidence_admitting(
        head: &str,
        admits: impl Fn(&CrawledUrl) -> bool,
    ) -> PageIdentityEvidence {
        let html = format!("<!doctype html><html><head>{head}</head><body></body></html>");
        page_identity_evidence(
            &CrawledUrl::try_from(PAGE).unwrap(),
            &Url::parse(PAGE).unwrap(),
            html.as_bytes(),
            admits,
        )
    }

    fn canonical_of(head: &str) -> Option<String> {
        evidence_of(head).canonical_link.map(|url| url.to_string())
    }

    #[test]
    fn should_read_an_absolute_or_relative_canonical_link() {
        for head in [
            r#"<link rel="canonical" href="https://frenchmetro.com/products/ladle" />"#,
            r#"<link href="/products/ladle" rel="Canonical">"#,
            r#"<link rel="canonical" href=" /products/ladle?utm_source=x#top ">"#,
        ] {
            assert_eq!(
                canonical_of(head),
                Some("https://frenchmetro.com/products/ladle".to_owned()),
                "{head}"
            );
        }
    }

    #[test]
    fn should_ignore_a_canonical_link_off_the_page_origin_or_unparsable() {
        for head in [
            r#"<link rel="canonical" href="https://other.example/products/ladle">"#,
            r#"<link rel="canonical" href="http://frenchmetro.com/products/ladle">"#,
            r#"<link rel="canonical" href="https://www.frenchmetro.com/products/ladle">"#,
            r#"<link rel="canonical" href="mailto:shop@frenchmetro.com">"#,
            r#"<link rel="canonical" href="https://[broken/products/ladle">"#,
        ] {
            assert_eq!(canonical_of(head), None, "{head}");
        }
    }

    #[test]
    fn should_ignore_conflicting_canonical_links() {
        assert_eq!(
            canonical_of(
                r#"<link rel="canonical" href="/products/ladle">
                   <link rel="canonical" href="/products/spoon">"#
            ),
            None
        );
        assert_eq!(
            canonical_of(
                r#"<link rel="canonical" href="/products/ladle">
                   <link rel="canonical" href="https://frenchmetro.com/products/ladle">"#
            ),
            Some("https://frenchmetro.com/products/ladle".to_owned())
        );
    }

    #[test]
    fn should_ignore_a_relative_link_on_a_page_with_a_base_element() {
        assert_eq!(
            canonical_of(
                r#"<base href="https://cdn.example/"><link rel="canonical" href="/products/ladle">"#
            ),
            None
        );
    }

    #[test]
    fn should_ignore_a_link_the_crawl_may_not_fetch() {
        let evidence = evidence_admitting(
            r#"<link rel="canonical" href="/products/ladle">
               <link rel="alternate" hreflang="de" href="/de/products/ladle">"#,
            |url| !url.as_url().path().contains("ladle"),
        );
        assert_eq!(evidence, PageIdentityEvidence::default());
    }

    #[test]
    fn should_read_language_alternates_only() {
        let evidence = evidence_of(
            r#"<link rel="alternate" hreflang="x-default" href="/products/ladle">
               <link rel="alternate" hreflang="en" href="/en/products/ladle">
               <link rel="alternate" type="application/rss+xml" href="/feed">"#,
        );
        assert_eq!(
            evidence
                .language_alternates
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![
                "https://frenchmetro.com/products/ladle",
                "https://frenchmetro.com/en/products/ladle",
            ]
        );
    }

    #[test]
    fn should_recognise_a_shopify_storefront_by_its_head() {
        assert_eq!(
            evidence_of(r#"<script>Shopify.shop = "frenchmetro.myshopify.com";</script>"#).platform,
            Some(ShopPlatform::Shopify)
        );
        assert_eq!(evidence_of("<title>Shop</title>").platform, None);
    }

    #[test]
    fn should_read_the_head_only() {
        let html = format!(
            "<html><head><title>x</title></head><body>{}<link rel=\"canonical\" href=\"/products/x\"> Shopify.shop</body></html>",
            "a".repeat(10)
        );
        let evidence = page_identity_evidence(
            &CrawledUrl::try_from(PAGE).unwrap(),
            &Url::parse(PAGE).unwrap(),
            html.as_bytes(),
            |_| true,
        );
        assert_eq!(evidence, PageIdentityEvidence::default());
    }
}
