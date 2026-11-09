//! `GET /api/listing-sources/{id}`: one ListingSource with the live configuration it crawls and
//! scrapes by. The contract is in `docs/crawler/operations.md` (review service).

use super::ListingSourceOverview;
use super::ReviewActor;
use crate::CrawlerReviewId;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use serde::Serialize;
use time::OffsetDateTime;

/// A ListingSource as its own console page shows it: its overview row and its live configuration.
#[derive(Debug, Serialize)]
pub struct ListingSourceDetail {
    #[serde(flatten)]
    pub overview: ListingSourceOverview,
    /// The live URL pattern; `None` while the ListingSource has none, undecided or decided to
    /// have no pattern.
    pub url_pattern: Option<UrlPatternOverview>,
    /// The live product schemas the scraper extracts with, in the order it applies them.
    pub product_schemas: Vec<ProductCssSelectorSchema>,
    /// The live removed-page templates.
    pub removed_page_schemas: Vec<RemovedPageSchema>,
    /// The latest approved product-schema review, which the live schemas come from unless a live
    /// edit or restore replaced them; `None` without one.
    pub live_schema_review_id: Option<CrawlerReviewId>,
    /// When the live schemas were last stored; `None` while none are.
    #[serde(with = "time::serde::rfc3339::option")]
    pub live_schemas_since: Option<OffsetDateTime>,
}

/// The live URL pattern of a ListingSource: what the spider classifies product URLs by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UrlPatternOverview {
    /// The regex a product URL matches.
    pub value: String,
    /// The regex that takes a URL out of the pattern's matches; `None` without one.
    pub exclude: Option<String>,
    /// The latest approved URL-pattern review, with when and by whom it was approved; `None`
    /// when no approval is recorded.
    pub approved_review_id: Option<CrawlerReviewId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub approved_at: Option<OffsetDateTime>,
    pub approved_by: Option<ReviewActor>,
}
