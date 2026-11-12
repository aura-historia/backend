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
