use std::sync::LazyLock;

use regex::Regex;

use crate::review::model::CrawlerReviewPage;

pub const INDEX_HTML: &str = include_str!("ui/index.html");
pub const APP_JS: &str = include_str!("ui/app.js");
pub const STYLES_CSS: &str = include_str!("ui/styles.css");
pub const INSPECTOR_JS: &str = include_str!("ui/inspector.js");

const INSPECTOR_CSS: &str = r#"
.__crawler_review_hover { outline: 3px solid #f59e0b !important; outline-offset: 2px !important; cursor: crosshair !important; }
.__crawler_review_selected { outline: 3px solid #2563eb !important; outline-offset: 2px !important; background-color: rgba(37, 99, 235, 0.10) !important; }
"#;

pub fn instrument_review_page(page: &CrawlerReviewPage, html: &str) -> String {
    instrument_live_html(&page.url, html)
}

pub fn instrument_live_html(base_url: &str, html: &str) -> String {
    let raw_html =
        expose_noscript_fallbacks(&disable_page_directives(&disable_existing_scripts(html)));
    let base = format!(
        r#"<base href="{}"><style>{}</style>"#,
        escape_html_attr(base_url),
        INSPECTOR_CSS
    );
    // The preview policy admits this script by the hash of `INSPECTOR_JS`, so the element
    // must hold exactly that text.
    let script = format!("<script>{INSPECTOR_JS}</script>");

    let head_end = head_insert_position(&raw_html);
    let with_head = format!("{}{base}{}", &raw_html[..head_end], &raw_html[head_end..]);

    if with_head.contains("</body>") {
        with_head.replacen("</body>", &format!("{script}</body>"), 1)
    } else if with_head.contains("</BODY>") {
        with_head.replacen("</BODY>", &format!("{script}</BODY>"), 1)
    } else {
        format!("{with_head}{script}")
    }
}

/// Where the injected `<base>` goes: right after the opening `<head>` tag, with or without
/// attributes, else after `<html>`, else after the doctype. Text before a doctype would
/// put the page into quirks mode and change its layout.
fn head_insert_position(html: &str) -> usize {
    let lower = html.to_ascii_lowercase();
    ["<head", "<html", "<!doctype"]
        .into_iter()
        .find_map(|tag| after_opening_tag(&lower, tag))
        .unwrap_or(0)
}

fn after_opening_tag(lower_html: &str, tag: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = lower_html[from..].find(tag) {
        let name_end = from + found + tag.len();
        match lower_html.as_bytes().get(name_end) {
            Some(&byte) if byte == b'>' || byte == b'/' || byte.is_ascii_whitespace() => {
                return Some(name_end + lower_html[name_end..].find('>')? + 1);
            }
            _ => from = name_end,
        }
    }
    None
}

fn expose_noscript_fallbacks(html: &str) -> String {
    html.replace("<noscript", r#"<div data-crawler-review-noscript="true""#)
        .replace("<NOSCRIPT", r#"<DIV data-crawler-review-noscript="true""#)
        .replace("</noscript>", "</div>")
        .replace("</NOSCRIPT>", "</DIV>")
}

/// Renames the attributes through which a page could act beyond its own rendering: a
/// `<meta http-equiv>` refresh would navigate the preview frame to the live shop page, out of
/// the preview policy, and a `<meta name="referrer">` or `referrerpolicy` attribute would
/// override the preview's `no-referrer`.
fn disable_page_directives(html: &str) -> String {
    static DIRECTIVES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i)(<[a-z][^<>]*?\s)(http-equiv|referrerpolicy|name\s*=\s*["']?\s*referrer\b)"#,
        )
        .unwrap_or_else(|error| panic!("{error}"))
    });
    DIRECTIVES
        .replace_all(html, "${1}data-crawler-review-disabled-$2")
        .into_owned()
}

fn disable_existing_scripts(html: &str) -> String {
    html.replace(
        "<script",
        r#"<script type="text/plain" data-crawler-review-disabled="true""#,
    )
    .replace(
        "<SCRIPT",
        r#"<SCRIPT type="text/plain" data-crawler-review-disabled="true""#,
    )
}

fn escape_html_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::{APP_JS, INSPECTOR_JS, instrument_live_html};
    use crate::scraper::plausibility::{CrossPageCheck, IdentityAgreementCheck, PlausibilityCheck};

    fn code(check: impl serde::Serialize) -> String {
        match serde_json::to_value(check) {
            Ok(serde_json::Value::String(code)) => code,
            other => panic!("a check must serialize as its code: {other:?}"),
        }
    }

    /// The console explains the per-page, cross-page and identity agreement check codes of
    /// the schema matrix itself, so each code must keep the crawler's description and none
    /// may go stale.
    #[test]
    fn should_describe_every_plausibility_check_code_as_the_crawler_does() {
        let checks: Vec<(String, &str)> = PlausibilityCheck::ALL
            .into_iter()
            .map(|check| (code(check), check.description()))
            .chain(
                CrossPageCheck::ALL
                    .into_iter()
                    .map(|check| (code(check), check.description())),
            )
            .chain(
                IdentityAgreementCheck::ALL
                    .into_iter()
                    .map(|check| (code(check), check.description())),
            )
            .collect();
        let console_descriptions = APP_JS
            .split_once("const plausibilityCheckDescriptions = {")
            .and_then(|(_, rest)| rest.split_once("};"))
            .map(|(entries, _)| entries)
            .unwrap_or_else(|| panic!("app.js must define plausibilityCheckDescriptions"));

        for (code, description) in &checks {
            assert!(
                console_descriptions.contains(&format!("{code}: '{description}'")),
                "app.js must describe {code} as '{description}'"
            );
        }
        assert_eq!(
            console_descriptions.matches(": '").count(),
            checks.len(),
            "app.js describes a check code the crawler does not report"
        );
    }

    const BASE: &str = r#"<base href="https://shop.example/p?a=1&amp;b=2">"#;

    #[test]
    fn should_inject_the_base_right_after_a_head_tag_with_attributes() {
        let html = r#"<!DOCTYPE html><html lang="de"><HEAD prefix="og: x"><title>T</title></HEAD><header></header><body></body></html>"#;

        let preview = instrument_live_html("https://shop.example/p?a=1&b=2", html);

        assert!(preview.starts_with(&format!(
            r#"<!DOCTYPE html><html lang="de"><HEAD prefix="og: x">{BASE}<style>"#
        )));
        assert_eq!(preview.matches("<base ").count(), 1);
    }

    #[test]
    fn should_keep_the_doctype_first_when_the_page_has_no_head() {
        let doctype_only =
            instrument_live_html("https://shop.example/p?a=1&b=2", "<!doctype html><p>x</p>");
        let fragment = instrument_live_html("https://shop.example/p?a=1&b=2", "<p>x</p>");

        assert!(doctype_only.starts_with(&format!("<!doctype html>{BASE}")));
        assert!(fragment.starts_with(BASE));
    }

    /// The preview policy admits the inspector by the hash of `INSPECTOR_JS`, so the injected
    /// element must hold exactly that text, and shop scripts must be inert.
    #[test]
    fn should_inject_the_inspector_verbatim_and_disable_shop_scripts() {
        let preview = instrument_live_html(
            "https://shop.example/",
            "<html><head></head><body><script src=\"/a.js\"></script></body></html>",
        );

        assert!(preview.contains(&format!("<script>{INSPECTOR_JS}</script></body>")));
        assert!(preview.contains(
            r#"<script type="text/plain" data-crawler-review-disabled="true" src="/a.js">"#
        ));
    }

    /// Shop scripts stay inert, so the inspector itself reveals lazy images from the attributes
    /// lazy-loading scripts read: instrumenting must leave those attributes intact, and the
    /// inspector must reveal each of them.
    #[test]
    fn should_keep_lazy_image_attributes_for_the_inspector_to_reveal() {
        let lazy_attributes = [
            ("data-src", "/a.jpg"),
            ("data-lazy-src", "/b.jpg"),
            ("data-original", "/c.jpg"),
            ("data-lazy", "/d.jpg"),
            ("data-srcset", "/e.webp 1x"),
            ("data-lazy-srcset", "/f.jpg 2x"),
        ];
        let images: String = lazy_attributes
            .iter()
            .map(|(name, value)| {
                format!(
                    r#"<img src="data:image/gif;base64,R0lGOD" {name}="{value}" loading="lazy">"#
                )
            })
            .collect();
        let preview = instrument_live_html(
            "https://shop.example/",
            &format!(
                r#"<html><head></head><body>{images}<picture><source data-srcset="/g.webp"><img data-src="/g.jpg"></picture><script src="/lazysizes.js"></script></body></html>"#
            ),
        );
        let revealed = INSPECTOR_JS
            .split_once("const lazyImageSources = {")
            .and_then(|(_, rest)| rest.split_once("};"))
            .map(|(entries, _)| entries)
            .unwrap_or_else(|| panic!("inspector.js must define lazyImageSources"));

        for (name, value) in lazy_attributes {
            assert!(preview.contains(&format!(r#"{name}="{value}""#)));
            assert!(
                revealed.contains(&format!("'{name}'")),
                "inspector.js must reveal {name}"
            );
        }
        assert!(preview.contains(r#"<source data-srcset="/g.webp">"#));
        assert!(preview.contains(
            r#"<script type="text/plain" data-crawler-review-disabled="true" src="/lazysizes.js">"#
        ));
    }

    #[test]
    fn should_not_take_a_header_element_for_the_head() {
        let preview = instrument_live_html(
            "https://shop.example/p?a=1&b=2",
            "<html><body><header>h</header></body></html>",
        );

        assert!(preview.starts_with(&format!("<html>{BASE}<style>")));
    }

    /// A refresh would navigate the preview frame to the live page, outside the preview
    /// policy; a referrer directive would override its `no-referrer`.
    #[test]
    fn should_disable_refresh_and_referrer_directives_of_the_shop_page() {
        let preview = instrument_live_html(
            "https://shop.example/",
            r#"<head><META HTTP-EQUIV="refresh" content="0;url=https://shop.example/x"><meta name="referrer" content="unsafe-url"><meta name="description" content="d"></head><body><p>the http-equiv name=referrer text</p><img referrerPolicy="unsafe-url" src="a.png"></body>"#,
        );

        assert!(preview.contains(r#"<META data-crawler-review-disabled-HTTP-EQUIV="refresh""#));
        assert!(preview.contains(r#"<meta data-crawler-review-disabled-name="referrer""#));
        assert!(preview.contains(r#"<meta name="description""#));
        assert!(preview.contains("<p>the http-equiv name=referrer text</p>"));
        assert!(
            preview.contains(r#"<img data-crawler-review-disabled-referrerPolicy="unsafe-url""#)
        );
    }
}
