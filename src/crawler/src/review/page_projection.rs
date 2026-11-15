//! A shop page as the review agents read it: the outline of the elements the schema generation
//! prompt shows the LLM ([`html_to_agent_page_outline`]), in CSS notation and with repeated
//! siblings counted, which costs the agents about half the context of that prompt's YAML, and
//! whether the page's own
//! structured data describes one schema.org `Product`, which the projection strips with its
//! scripts. An agent that reads the page's content rather than its elements asks for its markdown
//! ([`html_to_agent_page_markdown`]) instead of the outline, a fraction of its size. The markup decides nothing; it is information for whoever inspects the page
//! (`docs/crawler/extraction-and-review.md`, approval guard). The console's browser preview is
//! another view: the instrumented page of `/api/review-pages/{id}/inspect`.

use crate::review::field_candidates::field_candidates;
use crate::review::model::QueryValue;
use crate::review::product_markup::{JsonLdProductIds, describes_one_product, json_ld_product_ids};
use crate::scraper::css_selector::product_schema_service::{
    html_to_agent_page_markdown, html_to_agent_page_outline,
};
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

/// Which view of a page a projection shows: its outline, from which selectors are written, or its
/// markdown, for reading what it shows. The console's `view` query parameter names it in lower
/// case.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PageView {
    /// The elements, one per line in CSS notation with their text after ` | `.
    #[default]
    Outline,
    /// The readable content as markdown: headings, paragraphs, list items, table rows, the text of
    /// links and buttons and image alt text, without tags, attributes or URLs.
    Markdown,
    /// Per product-schema field, the few elements that could hold its value, in CSS notation with
    /// their values, and the page's JSON-LD values: a fraction of the outline, to write a schema
    /// from ([`field_candidates`]).
    Fields,
}

impl PageView {
    /// The value of the console's `view` query parameter that names this view.
    pub fn query_name(self) -> &'static str {
        Self::VALUES
            .iter()
            .find_map(|(name, view)| (*view == self).then_some(*name))
            .unwrap_or("outline")
    }
}

impl QueryValue for PageView {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("outline", Self::Outline),
        ("markdown", Self::Markdown),
        ("fields", Self::Fields),
    ];
}

/// The projected view of one page fetched live for a review, as
/// `GET /api/reviews/{id}/page-projection` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageProjection {
    pub review_id: CrawlerReviewId,
    /// The review page projected, or `None` for a URL of the review's ListingSource.
    pub review_page_id: Option<CrawlerReviewPageId>,
    /// The URL fetched.
    pub url: String,
    /// The URL the shop served the page from after redirects on its own host.
    pub final_url: String,
    /// The view `projection` shows.
    pub view: PageView,
    /// The page in `view`, without scripts, styles, navigation, header and footer and at most a
    /// bounded number of lines. The outline shows the elements, attributes and text the schema
    /// generation prompt shows, one element per line in CSS notation, with long texts and
    /// attributes shortened and runs of same-structure siblings counted; the markdown shows the
    /// readable content with whole texts.
    pub projection: String,
    /// Whether the page's JSON-LD or microdata describes exactly one top-level schema.org
    /// `Product`, as [`describes_one_product`] decides it. Information only.
    pub describes_one_product: bool,
    /// The identifiers of the page's JSON-LD `Product` that a `json_ld_product` rule reads,
    /// which the projection strips with the scripts.
    pub json_ld_product_ids: JsonLdProductIds,
}

/// A page of a ListingSource as the qualification of the source reads it:
/// `GET /api/source-qualifications/{id}/page` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourcePageProjection {
    pub listing_source_id: listing_source_core::ListingSourceId,
    /// The URL fetched.
    pub url: String,
    /// The URL the shop served the page from after redirects on its own host.
    pub final_url: String,
    pub view: PageView,
    /// The page in `view`, as [`PageProjection::projection`] describes it.
    pub projection: String,
    /// Whether the page's structured data describes exactly one product. Information only.
    pub describes_one_product: bool,
    /// The pages of the shop the page links, which the projection leaves out: where to read on.
    pub links: Vec<PageLink>,
}

/// The most links of a page the qualification reads.
pub const MAX_PAGE_LINKS: usize = 80;

/// The longest link text kept, in characters.
const MAX_LINK_TEXT_CHARS: usize = 80;

/// One page of the shop a page links, with the link's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageLink {
    pub url: String,
    pub text: String,
}

/// The distinct http(s) pages on `page_url`'s host or its www twin that `html` links, resolved as
/// Spider resolves them, without fragments, in page order, at most [`MAX_PAGE_LINKS`]; the page
/// itself is left out. A shop's own navigation names its category and item pages, which the
/// projection's views drop.
pub fn page_links(html: &str, page_url: &Url) -> Vec<PageLink> {
    static LINKS: std::sync::LazyLock<scraper::Selector> =
        std::sync::LazyLock::new(|| scraper::Selector::parse("a[href]").expect("link selector"));
    let base = crate::spider::discovery::link_base(page_url, html.as_bytes());
    let document = scraper::Html::parse_document(html);
    let mut seen = std::collections::HashSet::new();
    let mut page = page_url.clone();
    page.set_fragment(None);
    seen.insert(page.to_string());
    document
        .select(&LINKS)
        .filter_map(|anchor| {
            let mut url = base.join(anchor.value().attr("href")?.trim()).ok()?;
            url.set_fragment(None);
            let on_shop = matches!(url.scheme(), "http" | "https")
                && crate::network::policy::is_same_or_www_host(&url, page_url);
            (on_shop && seen.insert(url.to_string())).then(|| PageLink {
                text: anchor
                    .text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(MAX_LINK_TEXT_CHARS)
                    .collect(),
                url: url.to_string(),
            })
        })
        .take(MAX_PAGE_LINKS)
        .collect()
}

/// What a page's `html` shows the review agents: its projection in one view, and whether its
/// structured data describes one product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedHtml {
    pub view: PageView,
    pub projection: String,
    pub describes_one_product: bool,
    pub json_ld_product_ids: JsonLdProductIds,
}

impl ProjectedHtml {
    /// Projects `html` in `view`. Parsing a shop's page costs more than its size on deeply nested
    /// markup, so callers on the async runtime run this on a blocking thread.
    pub fn of(html: &str, view: PageView) -> Self {
        Self {
            view,
            projection: match view {
                PageView::Outline => html_to_agent_page_outline(html),
                PageView::Markdown => html_to_agent_page_markdown(html),
                PageView::Fields => field_candidates(html),
            },
            describes_one_product: describes_one_product(html),
            json_ld_product_ids: json_ld_product_ids(html),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The links a qualifier reads on: the shop's own pages, resolved against the page's base,
    /// once each, without the page itself, other hosts, fragments or non-http links.
    #[test]
    fn should_list_the_shop_pages_a_page_links_once_each() {
        let html = r##"<html><head><base href="https://www.shop.example/de/"></head><body>
            <a href="/">Home</a>
            <a href="moebel/">  Möbel
               und Lampen </a>
            <a href="moebel/#top">again</a>
            <a href="https://shop.example/artikel/1">Art. 1</a>
            <a href="https://elsewhere.example/">other shop</a>
            <a href="mailto:info@shop.example">mail</a>
            <a href="#cart">cart</a>
        </body></html>"##;

        let links = page_links(html, &Url::parse("https://www.shop.example/de/").unwrap());

        assert_eq!(
            links
                .iter()
                .map(|link| (link.url.as_str(), link.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("https://www.shop.example/", "Home"),
                ("https://www.shop.example/de/moebel/", "Möbel und Lampen"),
                ("https://shop.example/artikel/1", "Art. 1"),
            ]
        );
    }

    #[test]
    fn should_outline_the_page_and_report_its_product_markup() {
        let html = r#"<!doctype html><html><head>
            <script type="application/ld+json">{"@context":"https://schema.org","@type":"Product","name":"Oak chair"}</script>
            </head><body><nav>Menu</nav><h1 class="product-title">Oak chair</h1>
            <span class="price">£120</span></body></html>"#;

        let projected = ProjectedHtml::of(html, PageView::Outline);

        assert_eq!(projected.projection, html_to_agent_page_outline(html));
        assert!(
            projected
                .projection
                .contains("h1.product-title | Oak chair")
        );
        assert!(projected.projection.contains("Oak chair"));
        assert!(!projected.projection.contains("ld+json"));
        assert!(!projected.projection.contains("Menu"));
        assert!(projected.describes_one_product);
    }

    #[test]
    fn should_show_the_json_ld_product_identifiers_the_projection_strips() {
        let html = r#"<html><head><script type="application/ld+json">
            {"@type":"Product","productID":"9441317814427","sku":"R-1"}</script></head>
            <body><h1>Ring</h1></body></html>"#;

        let ids = ProjectedHtml::of(html, PageView::Outline).json_ld_product_ids;

        assert_eq!(ids.product_id.as_deref(), Some("9441317814427"));
        assert_eq!(ids.sku.as_deref(), Some("R-1"));
        assert_eq!(
            ProjectedHtml::of("<html><body><h1>Ring</h1></body></html>", PageView::Outline)
                .json_ld_product_ids,
            JsonLdProductIds::default()
        );
    }

    #[test]
    fn should_report_no_product_markup_on_a_page_without_structured_data() {
        let projected = ProjectedHtml::of(
            "<html><body><h1>Our journal</h1></body></html>",
            PageView::Outline,
        );

        assert!(!projected.describes_one_product);
        assert!(projected.projection.contains("Our journal"));
    }

    #[test]
    fn should_project_the_markdown_view_with_the_same_product_markup() {
        let html = r#"<html><head><script type="application/ld+json">{"@type":"Product","name":"Oak chair","sku":"C-1"}</script></head>
            <body><nav>Menu</nav><h1 class="product-title">Oak chair</h1><span class="price">£120</span></body></html>"#;

        let markdown = ProjectedHtml::of(html, PageView::Markdown);
        let outline = ProjectedHtml::of(html, PageView::Outline);

        assert_eq!(markdown.view, PageView::Markdown);
        assert_eq!(markdown.projection, "# Oak chair\n£120\n");
        assert!(markdown.describes_one_product);
        assert_eq!(markdown.json_ld_product_ids.sku.as_deref(), Some("C-1"));
        assert_eq!(
            (markdown.describes_one_product, markdown.json_ld_product_ids),
            (outline.describes_one_product, outline.json_ld_product_ids)
        );
        assert_eq!(outline.view, PageView::Outline);
    }

    #[test]
    fn should_name_each_view_in_lower_case_in_the_query_and_in_screaming_snake_case_in_json() {
        assert_eq!(PageView::parse("markdown"), Some(PageView::Markdown));
        assert_eq!(PageView::parse("outline"), Some(PageView::Outline));
        assert_eq!(PageView::parse("MARKDOWN"), None);
        assert_eq!(PageView::parse("fields"), Some(PageView::Fields));
        assert_eq!(PageView::accepted(), "outline, markdown, fields");
        for view in [PageView::Outline, PageView::Markdown, PageView::Fields] {
            assert_eq!(PageView::parse(view.query_name()), Some(view));
        }
        assert_eq!(
            serde_json::to_value(PageView::Markdown).ok(),
            Some(serde_json::json!("MARKDOWN"))
        );
        assert_eq!(
            serde_json::from_value::<PageView>(serde_json::json!("OUTLINE")).ok(),
            Some(PageView::Outline)
        );
        assert!(serde_json::from_value::<PageView>(serde_json::json!("markdown")).is_err());
        assert_eq!(PageView::default(), PageView::Outline);
    }
}
