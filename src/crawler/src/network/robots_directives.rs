//! Page-level robots directives a shop sends next to robots.txt: the `X-Robots-Tag` response
//! header and `<meta name="robots">` in the page's `<head>`.
//!
//! A directive that forbids indexing or AI use is a technical notice the crawler honours like a
//! robots.txt `Disallow`: the page is not captured. Directives apply when they are unscoped or
//! scoped to the crawler's product token; those scoped to another bot are ignored. Every
//! applicable directive from every header and tag counts and none overrides another, so the
//! outcome fails closed. The contract is in `docs/crawler/networking.md` (page robots
//! directives).

use reqwest::header::HeaderMap;
use scraper::{Html, Selector};
use std::sync::LazyLock;

/// The product token a directive is scoped to; the same token robots.txt groups address.
const CRAWLER_PRODUCT_TOKEN: &str = "aurahistoriabot";

/// What a page's directives can forbid the crawler to do. `none` is `noindex, nofollow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restriction {
    Capture,
    Following,
}

impl Restriction {
    const fn directives(self) -> &'static [&'static str] {
        match self {
            Self::Capture => &["noindex", "none", "noai"],
            Self::Following => &["nofollow", "none"],
        }
    }

    fn forbidden_by(self, token: &str) -> Option<&'static str> {
        self.directives()
            .iter()
            .copied()
            .find(|directive| *directive == token)
    }
}

/// Directives that only describe how a page may be shown or followed, so that a `name: value`
/// token starting with one of them is a directive and no bot name.
const OTHER_DIRECTIVES: [&str; 12] = [
    "all",
    "index",
    "follow",
    "nofollow",
    "noarchive",
    "nosnippet",
    "noimageindex",
    "noimageai",
    "notranslate",
    "indexifembedded",
    "unavailable_after",
    "max-snippet",
];

static META_ROBOTS: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("meta[name][content]").unwrap_or_else(|error| panic!("{error:?}"))
});

/// Where a forbidding directive was found, for logs and failure details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectiveRefusal {
    pub directive: &'static str,
    pub source: DirectiveSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveSource {
    Header,
    Meta,
}

impl DirectiveRefusal {
    /// The words a fetch failure names the refusal by.
    pub fn describe(&self) -> String {
        let source = match self.source {
            DirectiveSource::Header => "X-Robots-Tag header",
            DirectiveSource::Meta => "meta robots tag",
        };
        format!("{source} forbids capture ({})", self.directive)
    }
}

/// The directive of the response's `X-Robots-Tag` headers that forbids capturing the page for
/// the crawler. Judged before the body is read, so a refused page is not downloaded.
pub fn refusal_for_headers(headers: &HeaderMap) -> Option<DirectiveRefusal> {
    forbidding_header_directive(headers, Restriction::Capture).map(|directive| DirectiveRefusal {
        directive,
        source: DirectiveSource::Header,
    })
}

/// The directive of the page's meta robots tags that forbids capturing it for the crawler.
pub fn refusal_for_meta(html: &str) -> Option<DirectiveRefusal> {
    forbidding_meta_directive(html, Restriction::Capture).map(|directive| DirectiveRefusal {
        directive,
        source: DirectiveSource::Meta,
    })
}

fn forbidding_header_directive(
    headers: &HeaderMap,
    restriction: Restriction,
) -> Option<&'static str> {
    headers.get_all("x-robots-tag").iter().find_map(|value| {
        forbidding_in_header_value(&String::from_utf8_lossy(value.as_bytes()), restriction)
    })
}

/// A header value is a comma-separated list of directives, each optionally opening a scope with
/// `<bot>: <directive>`; the scope lasts until the next one. A date after `unavailable_after:`
/// contains no directive and is skipped.
fn forbidding_in_header_value(value: &str, restriction: Restriction) -> Option<&'static str> {
    let mut applies = true;
    for token in value.split(',') {
        let mut token = token.trim().to_ascii_lowercase();
        if let Some((prefix, rest)) = token.split_once(':') {
            let prefix = prefix.trim();
            if !OTHER_DIRECTIVES.contains(&prefix) {
                applies = prefix == CRAWLER_PRODUCT_TOKEN;
                token = rest.trim().to_owned();
            }
        }
        if applies && let Some(directive) = restriction.forbidden_by(&token) {
            return Some(directive);
        }
    }
    None
}

fn forbidding_meta_directive(html: &str, restriction: Restriction) -> Option<&'static str> {
    let head = Html::parse_document(head_of(html));
    head.select(&META_ROBOTS)
        .filter(|meta| {
            meta.value().attr("name").is_some_and(|name| {
                let name = name.trim().to_ascii_lowercase();
                name == "robots" || name == CRAWLER_PRODUCT_TOKEN
            })
        })
        .filter_map(|meta| meta.value().attr("content"))
        .flat_map(|content| content.split(','))
        .find_map(|token| restriction.forbidden_by(&token.trim().to_ascii_lowercase()))
}

/// The page up to the end of its `<head>`, which is all a meta robots tag may sit in; the whole
/// page when no end tag is found.
fn head_of(html: &str) -> &str {
    const END_TAG: &[u8] = b"</head";
    html.as_bytes()
        .windows(END_TAG.len())
        .position(|window| window.eq_ignore_ascii_case(END_TAG))
        .map_or(html, |end| &html[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(
                "x-robots-tag",
                HeaderValue::from_str(value).unwrap_or_else(|error| panic!("{error}")),
            );
        }
        headers
    }

    fn page(head: &str) -> String {
        format!("<html><head><title>x</title>{head}</head><body><h1>Vase</h1></body></html>")
    }

    fn header_refusal(values: &[&str]) -> Option<&'static str> {
        refusal_for_headers(&headers(values)).map(|refusal| refusal.directive)
    }

    fn meta_refusal(head: &str) -> Option<&'static str> {
        refusal_for_meta(&page(head)).map(|refusal| refusal.directive)
    }

    #[test]
    fn should_refuse_when_an_unscoped_header_says_noindex() {
        assert_eq!(header_refusal(&["noindex, nofollow"]), Some("noindex"));
        assert_eq!(header_refusal(&["NoIndex"]), Some("noindex"));
        assert_eq!(header_refusal(&["none"]), Some("none"));
        assert_eq!(header_refusal(&["noai"]), Some("noai"));
    }

    #[test]
    fn should_refuse_when_the_header_is_scoped_to_the_crawler() {
        assert_eq!(
            header_refusal(&["AuraHistoriaBot: noindex"]),
            Some("noindex")
        );
        assert_eq!(
            header_refusal(&["otherbot: index, aurahistoriabot: noai"]),
            Some("noai")
        );
    }

    #[test]
    fn should_ignore_a_header_scoped_to_another_bot() {
        assert_eq!(header_refusal(&["googlebot: noindex"]), None);
        assert_eq!(
            header_refusal(&["googlebot: noindex, nofollow, aurahistoriabot: index"]),
            None
        );
    }

    #[test]
    fn should_judge_every_header_when_the_response_repeats_it() {
        assert_eq!(header_refusal(&["index", "noindex"]), Some("noindex"));
    }

    #[test]
    fn should_allow_when_a_header_only_limits_presentation() {
        assert_eq!(
            header_refusal(&["noarchive, nosnippet, max-snippet: 20, noimageindex"]),
            None
        );
        assert_eq!(
            header_refusal(&["unavailable_after: 25 Jun 2030 15:00:00 PST"]),
            None
        );
    }

    #[test]
    fn should_refuse_when_a_meta_robots_tag_says_noindex() {
        assert_eq!(
            meta_refusal(r#"<meta name="robots" content="noindex, follow">"#),
            Some("noindex")
        );
        assert_eq!(
            meta_refusal(r#"<META NAME="Robots" CONTENT="NONE">"#),
            Some("none")
        );
        assert_eq!(
            meta_refusal(r#"<meta name="AuraHistoriaBot" content="noai">"#),
            Some("noai")
        );
    }

    #[test]
    fn should_ignore_a_meta_tag_for_another_bot_or_another_name() {
        assert_eq!(
            meta_refusal(r#"<meta name="googlebot" content="noindex">"#),
            None
        );
        assert_eq!(
            meta_refusal(r#"<meta name="description" content="noindex">"#),
            None
        );
        assert_eq!(
            meta_refusal(r#"<meta name="robots" content="index, follow">"#),
            None
        );
    }

    #[test]
    fn should_not_read_a_robots_tag_that_sits_after_the_head() {
        let html = r#"<html><head><title>x</title></head><body><p>meta name="robots" content="noindex"</p></body></html>"#;

        assert_eq!(refusal_for_meta(html), None);
    }

    #[test]
    fn should_name_the_source_of_the_refusal() {
        let from_header = refusal_for_headers(&headers(&["noindex"]));
        let from_meta = refusal_for_meta(&page(r#"<meta name="robots" content="noindex">"#));

        assert_eq!(
            from_header.map(|refusal| refusal.describe()),
            Some("X-Robots-Tag header forbids capture (noindex)".to_owned())
        );
        assert_eq!(
            from_meta.map(|refusal| refusal.describe()),
            Some("meta robots tag forbids capture (noindex)".to_owned())
        );
    }
}
