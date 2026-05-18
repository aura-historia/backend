use crate::review::model::CrawlerReviewPage;

pub const INDEX_HTML: &str = include_str!("ui/index.html");
pub const APP_JS: &str = include_str!("ui/app.js");
pub const STYLES_CSS: &str = include_str!("ui/styles.css");
pub const INSPECTOR_JS: &str = include_str!("ui/inspector.js");

const INSPECTOR_CSS: &str = r#"
.__crawler_review_hover { outline: 3px solid #f59e0b !important; outline-offset: 2px !important; cursor: crosshair !important; }
.__crawler_review_selected { outline: 3px solid #2563eb !important; outline-offset: 2px !important; background-color: rgba(37, 99, 235, 0.10) !important; }
"#;

pub fn instrument_review_page(page: &CrawlerReviewPage) -> String {
    let raw_html = expose_noscript_fallbacks(&disable_existing_scripts(&page.raw_html));
    let base = format!(
        r#"<base href="{}"><style>{}</style>"#,
        escape_html_attr(&page.url),
        INSPECTOR_CSS
    );
    let script = format!("<script>{INSPECTOR_JS}</script>");

    let with_head = if raw_html.contains("<head>") {
        raw_html.replacen("<head>", &format!("<head>{base}"), 1)
    } else if raw_html.contains("<HEAD>") {
        raw_html.replacen("<HEAD>", &format!("<HEAD>{base}"), 1)
    } else {
        format!("{base}{raw_html}")
    };

    if with_head.contains("</body>") {
        with_head.replacen("</body>", &format!("{script}</body>"), 1)
    } else if with_head.contains("</BODY>") {
        with_head.replacen("</BODY>", &format!("{script}</BODY>"), 1)
    } else {
        format!("{with_head}{script}")
    }
}

fn expose_noscript_fallbacks(html: &str) -> String {
    html.replace("<noscript", r#"<div data-crawler-review-noscript="true""#)
        .replace("<NOSCRIPT", r#"<DIV data-crawler-review-noscript="true""#)
        .replace("</noscript>", "</div>")
        .replace("</NOSCRIPT>", "</DIV>")
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
