//! The `<base>` element of a fetched page, which says where its relative links lead
//! (`docs/crawler/networking.md#destination-policy`).
//!
//! Spider resolves the links it follows against the page's first absolute `<base href>` and
//! ignores a relative or unparsable one, so the crawler reads the element the same way wherever it
//! needs to know where a page's links lead.

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

use crate::network::page_head::head_of;

/// How a `<base>` element starts, in any letter case.
const BASE_TAG_START: &[u8] = b"<base";

static BASE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("base[href]").expect("base selector"));

/// Whether `document` has a `<base>` element with an `href`, absolute or not.
pub(super) fn has_base(document: &Html) -> bool {
    document.select(&BASE).next().is_some()
}

/// The first absolute `<base href>` of `document`, the one Spider resolves the page's links
/// against.
pub(super) fn absolute_base(document: &Html) -> Option<Url> {
    document
        .select(&BASE)
        .find_map(|base| Url::parse(base.value().attr("href")?.trim()).ok())
}

/// What Spider resolves the links of the page fetched as `page_url` with body `html` against: the
/// page's first absolute `<base href>` when it has one, otherwise the page URL itself. Spider never
/// follows a redirect itself, so a page with links was served at the URL it was fetched as. Only
/// the page's `<head>` is read, and parsed only when it names a `<base>` element at all.
pub(super) fn link_base(page_url: &Url, html: &[u8]) -> Url {
    let head = head_of(html);
    let names_base = head
        .windows(BASE_TAG_START.len())
        .any(|window| window.eq_ignore_ascii_case(BASE_TAG_START));
    names_base
        .then(|| absolute_base(&Html::parse_document(&String::from_utf8_lossy(head))))
        .flatten()
        .unwrap_or_else(|| page_url.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(head: &str) -> Html {
        Html::parse_document(&format!("<html><head>{head}</head><body></body></html>"))
    }

    #[test]
    fn should_take_the_first_absolute_base_and_skip_relative_or_unparsable_ones() {
        let head = r#"<base href="/relative/"><base href="https://[broken/">
                      <base href=" https://www.shop.example/ "><base href="https://other.example/">"#;

        assert_eq!(
            absolute_base(&document(head)).map(String::from).as_deref(),
            Some("https://www.shop.example/")
        );
        assert!(has_base(&document(head)));
    }

    #[test]
    fn should_find_no_base_on_a_page_with_only_a_relative_base_or_none() {
        assert_eq!(
            absolute_base(&document(r#"<base href="/relative/">"#)),
            None
        );
        assert!(has_base(&document(r#"<base href="/relative/">"#)));
        assert_eq!(absolute_base(&document("<title>x</title>")), None);
        assert!(!has_base(&document(r#"<base target="_blank">"#)));
    }

    #[test]
    fn should_resolve_links_against_the_page_s_absolute_base_or_else_the_page_url() {
        let page =
            Url::parse("https://www.shop.example/en-GB/latest-items_pg_904/page_904").unwrap();
        let base_of = |html: &str| String::from(link_base(&page, html.as_bytes()));

        assert_eq!(
            base_of(r#"<html><HEAD><BASE HREF="https://www.shop.example/"></HEAD><body>"#),
            "https://www.shop.example/"
        );
        for html in [
            "<html><head><title>x</title></head><body></body></html>",
            r#"<html><head><base href="/relative/"></head></html>"#,
            r#"<html><head></head><body><base href="https://other.example/"></body></html>"#,
            "",
        ] {
            assert_eq!(base_of(html), page.as_str(), "{html}");
        }
    }
}
