//! The `<base>` element of a fetched page, which says where its relative links lead
//! (`docs/crawler/networking.md#destination-policy`).
//!
//! Spider resolves the links it follows against the page's first absolute `<base href>` and
//! ignores a relative or unparsable one, so the crawler reads the element the same way wherever it
//! needs to know where a page's links lead.

use scraper::{Html, Selector};
use std::sync::LazyLock;
use url::Url;

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
}
