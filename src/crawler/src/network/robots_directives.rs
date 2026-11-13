//! Page-level robots directives a shop sends next to robots.txt: the `X-Robots-Tag` response
//! header and `<meta name="robots">` in the page's `<head>`.
//!
//! A directive that forbids indexing or AI use is a technical notice the crawler honours like a
//! robots.txt `Disallow`: the page is not captured. Directives apply when they are unscoped or
//! scoped to the crawler's product token; those scoped to another bot are ignored. Every
//! applicable directive from every header and tag counts and none overrides another, so the
//! outcome fails closed. The contract is in `docs/crawler/networking.md` (page robots
//! directives).

use crate::network::page_head::head_of;
use crate::network::robots::{CRAWLER_PRODUCT_TOKEN, product_token};
use reqwest::header::HeaderMap;
use scraper::{Html, Selector};
use std::sync::LazyLock;

/// Directives that forbid capturing a page. `none` is `noindex, nofollow`.
const FORBIDDING_CAPTURE: [&str; 3] = ["noindex", "none", "noai"];

/// Directives that forbid following the page's links.
const FORBIDDING_FOLLOWING: [&str; 2] = ["nofollow", "none"];

/// Every directive search engines document, so that a `name: value` token starting with one of
/// them is read as that directive with its value and never as a bot's scope.
const KNOWN_DIRECTIVES: [&str; 17] = [
    "all",
    "index",
    "noindex",
    "none",
    "noai",
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
    "max-image-preview",
    "max-video-preview",
];

/// The directive whose value is a date, which may itself contain a comma.
const UNAVAILABLE_AFTER: &str = "unavailable_after";

static META_TAGS: LazyLock<Selector> = LazyLock::new(|| {
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

/// What a page's applicable robots directives forbid the crawler.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageDirectives {
    refusal: Option<DirectiveRefusal>,
    forbids_following: bool,
}

impl PageDirectives {
    /// The directives of the response's `X-Robots-Tag` headers, which can be judged before the
    /// body is read.
    pub fn of_headers(headers: &HeaderMap) -> Self {
        headers
            .get_all("x-robots-tag")
            .iter()
            .map(|value| {
                Self::from_applicable(
                    applicable_header_directives(&String::from_utf8_lossy(value.as_bytes())),
                    DirectiveSource::Header,
                )
            })
            .fold(Self::default(), Self::and)
    }

    /// The directives of the meta robots tags in the `<head>` of `html`. Only the head is read,
    /// and parsed only when it names robots or the crawler at all.
    pub fn of_html(html: &[u8]) -> Self {
        let head = String::from_utf8_lossy(head_of(html));
        let lowercase_head = head.to_ascii_lowercase();
        if !lowercase_head.contains("robots") && !lowercase_head.contains(CRAWLER_PRODUCT_TOKEN) {
            return Self::default();
        }
        let head = Html::parse_document(&head);
        let directives = head
            .select(&META_TAGS)
            .filter(|meta| meta.value().attr("name").is_some_and(addresses_crawler))
            .filter_map(|meta| meta.value().attr("content"))
            .flat_map(|content| content.split(','))
            .map(|token| directive_name(&token.trim().to_ascii_lowercase()).to_owned())
            .collect();
        Self::from_applicable(directives, DirectiveSource::Meta)
    }

    /// The directives of a whole page: its headers and its meta robots tags together.
    pub fn of_page(headers: &HeaderMap, html: &[u8]) -> Self {
        Self::of_headers(headers).and(Self::of_html(html))
    }

    /// The directive that forbids capturing the page, if any applies.
    pub fn refusal(&self) -> Option<&DirectiveRefusal> {
        self.refusal.as_ref()
    }

    /// Whether an applicable directive forbids following the page's links.
    pub fn forbids_following(&self) -> bool {
        self.forbids_following
    }

    fn from_applicable(directives: Vec<String>, source: DirectiveSource) -> Self {
        let refusal = directives.iter().find_map(|directive| {
            FORBIDDING_CAPTURE
                .into_iter()
                .find(|forbidding| forbidding == directive)
                .map(|directive| DirectiveRefusal { directive, source })
        });
        let forbids_following = directives
            .iter()
            .any(|directive| FORBIDDING_FOLLOWING.contains(&directive.as_str()));
        Self {
            refusal,
            forbids_following,
        }
    }

    /// Both sets together: no directive overrides another, and the first refusal found names it.
    fn and(self, other: Self) -> Self {
        Self {
            refusal: self.refusal.or(other.refusal),
            forbids_following: self.forbids_following || other.forbids_following,
        }
    }
}

/// The names of the directives of one header value that apply to the crawler. The value is a
/// comma-separated list of directives, each optionally opening a scope with `<bot>: <directive>`
/// that lasts until the next one. A token opens a scope only when the text before its colon is a
/// bot name and no known directive, such as `max-image-preview: large`. The date of
/// `unavailable_after` may contain a comma, so a token right after it that starts with a digit
/// is the rest of that date.
fn applicable_header_directives(value: &str) -> Vec<String> {
    let mut applies = true;
    let mut after_unavailable_after = false;
    let mut directives = Vec::new();
    for token in value.split(',') {
        let token = token.trim().to_ascii_lowercase();
        if std::mem::take(&mut after_unavailable_after)
            && token.starts_with(|first: char| first.is_ascii_digit())
        {
            continue;
        }
        let directive = match token.split_once(':') {
            Some((prefix, rest)) if opens_scope(prefix.trim()) => {
                applies = product_token(prefix.trim()) == CRAWLER_PRODUCT_TOKEN;
                rest.trim()
            }
            _ => token.as_str(),
        };
        let name = directive_name(directive);
        after_unavailable_after = name == UNAVAILABLE_AFTER;
        if applies && !name.is_empty() {
            directives.push(name.to_owned());
        }
    }
    directives
}

/// The name of a directive token, without the value a `name: value` directive carries.
fn directive_name(token: &str) -> &str {
    token.split(':').next().unwrap_or_default().trim()
}

/// Whether the text before a colon names a bot: a known directive never does, and a bot name
/// starts with a letter and holds no space, as a date's time does.
fn opens_scope(prefix: &str) -> bool {
    !KNOWN_DIRECTIVES.contains(&prefix)
        && prefix.starts_with(|first: char| first.is_ascii_alphabetic())
        && prefix
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.' | '/'))
}

/// Whether a meta tag's `name` addresses the crawler: every robot, or the crawler's product token.
fn addresses_crawler(name: &str) -> bool {
    let name = name.trim();
    name.eq_ignore_ascii_case("robots") || product_token(name) == CRAWLER_PRODUCT_TOKEN
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
        PageDirectives::of_headers(&headers(values))
            .refusal()
            .map(|refusal| refusal.directive)
    }

    fn meta_refusal(head: &str) -> Option<&'static str> {
        PageDirectives::of_html(page(head).as_bytes())
            .refusal()
            .map(|refusal| refusal.directive)
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
    fn should_refuse_when_the_header_scope_names_the_crawler_with_its_version() {
        assert_eq!(
            header_refusal(&["AuraHistoriaBot/1.0: noindex"]),
            Some("noindex")
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
    fn should_not_read_a_valued_directive_as_a_bot_scope() {
        assert_eq!(
            header_refusal(&["max-image-preview:large, noindex"]),
            Some("noindex")
        );
        assert_eq!(
            header_refusal(&["max-video-preview: -1, noindex"]),
            Some("noindex")
        );
    }

    #[test]
    fn should_read_past_an_unavailable_after_date_that_contains_a_comma() {
        assert_eq!(
            header_refusal(&["unavailable_after: Saturday, 25-Jun-2030 15:00:00 GMT, noindex"]),
            Some("noindex")
        );
        assert_eq!(
            header_refusal(&["unavailable_after: Sat, 25 Jun 2030 15:00:00 GMT, noindex"]),
            Some("noindex")
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
        assert_eq!(
            meta_refusal(r#"<meta name="robots" content="max-image-preview:large, noindex">"#),
            Some("noindex")
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
        let html = r#"<html><head><title>x</title></head><body><meta name="robots" content="noindex"></body></html>"#;

        assert_eq!(PageDirectives::of_html(html.as_bytes()).refusal(), None);
    }

    #[test]
    fn should_join_header_and_meta_directives_of_a_page() {
        let html = page(r#"<meta name="robots" content="nofollow">"#);

        let directives = PageDirectives::of_page(&headers(&["noai"]), html.as_bytes());

        assert_eq!(
            directives.refusal(),
            Some(&DirectiveRefusal {
                directive: "noai",
                source: DirectiveSource::Header,
            })
        );
        assert!(directives.forbids_following());
        assert!(!PageDirectives::of_headers(&headers(&["noindex"])).forbids_following());
        assert!(PageDirectives::of_headers(&headers(&["none"])).forbids_following());
    }

    #[test]
    fn should_name_the_source_of_the_refusal() {
        let from_header = PageDirectives::of_headers(&headers(&["noindex"]));
        let from_meta =
            PageDirectives::of_html(page(r#"<meta name="robots" content="noindex">"#).as_bytes());

        assert_eq!(
            from_header.refusal().map(DirectiveRefusal::describe),
            Some("X-Robots-Tag header forbids capture (noindex)".to_owned())
        );
        assert_eq!(
            from_meta.refusal().map(DirectiveRefusal::describe),
            Some("meta robots tag forbids capture (noindex)".to_owned())
        );
    }

    fn following_forbidden(values: &[&str], head: &str) -> bool {
        PageDirectives::of_page(&headers(values), page(head).as_bytes()).forbids_following()
    }

    #[test]
    fn should_forbid_following_when_a_header_or_meta_tag_says_nofollow_or_none() {
        assert!(following_forbidden(&["nofollow"], ""));
        assert!(following_forbidden(&["aurahistoriabot: none"], ""));
        assert!(following_forbidden(
            &[],
            r#"<meta name="robots" content="index, nofollow">"#
        ));
        assert!(following_forbidden(
            &[],
            r#"<meta name="AuraHistoriaBot" content="none">"#
        ));
    }

    #[test]
    fn should_allow_following_when_the_directive_is_for_another_bot_or_only_noindex() {
        assert!(!following_forbidden(&["googlebot: nofollow"], ""));
        assert!(!following_forbidden(
            &["noindex"],
            r#"<meta name="robots" content="noindex">"#
        ));
        assert!(!following_forbidden(
            &[],
            r#"<meta name="googlebot" content="nofollow">"#
        ));
        assert!(!following_forbidden(&[], ""));
    }
}
