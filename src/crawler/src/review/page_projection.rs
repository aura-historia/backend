//! A shop page as the review agents read it: the YAML projection the schema generation prompt
//! gives the LLM ([`html_to_schema_prompt_dsl`]), reused as is, and whether the page's own
//! structured data describes one schema.org `Product`, which the projection strips with its
//! scripts. The markup decides nothing; it is information for whoever inspects the page
//! (`docs/crawler/extraction-and-review.md`, approval guard). The console's browser preview is
//! another view: the instrumented page of `/api/review-pages/{id}/inspect`.

use crate::review::product_markup::describes_one_product;
use crate::scraper::css_selector::product_schema_service::html_to_schema_prompt_dsl;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::Serialize;

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
    /// The page as YAML: the elements, attributes and text the schema generation prompt shows,
    /// without scripts, styles, navigation, header and footer, with long texts and attributes
    /// shortened and at most a bounded number of elements.
    pub projection: String,
    /// Whether the page's JSON-LD or microdata describes exactly one top-level schema.org
    /// `Product`, as [`describes_one_product`] decides it. Information only.
    pub describes_one_product: bool,
}

/// What a page's `html` shows the review agents: its projection, and whether its structured data
/// describes one product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedHtml {
    pub projection: String,
    pub describes_one_product: bool,
}

impl ProjectedHtml {
    /// Projects `html`. Parsing a shop's page costs more than its size on deeply nested markup,
    /// so callers on the async runtime run this on a blocking thread.
    pub fn of(html: &str) -> Self {
        Self {
            projection: html_to_schema_prompt_dsl(html),
            describes_one_product: describes_one_product(html),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_project_the_page_as_the_schema_prompt_does_and_report_its_product_markup() {
        let html = r#"<!doctype html><html><head>
            <script type="application/ld+json">{"@context":"https://schema.org","@type":"Product","name":"Oak chair"}</script>
            </head><body><nav>Menu</nav><h1 class="product-title">Oak chair</h1>
            <span class="price">£120</span></body></html>"#;

        let projected = ProjectedHtml::of(html);

        assert_eq!(projected.projection, html_to_schema_prompt_dsl(html));
        assert!(projected.projection.contains("Oak chair"));
        assert!(!projected.projection.contains("ld+json"));
        assert!(!projected.projection.contains("Menu"));
        assert!(projected.describes_one_product);
    }

    #[test]
    fn should_report_no_product_markup_on_a_page_without_structured_data() {
        let projected = ProjectedHtml::of("<html><body><h1>Our journal</h1></body></html>");

        assert!(!projected.describes_one_product);
        assert!(projected.projection.contains("Our journal"));
    }
}
