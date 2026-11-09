//! `GET /api/listing_sources/{id}`: one ListingSource with its review history and what it
//! crawls and scrapes by. The contract is in `docs/crawler/operations.md` (review service).

use super::{ListingSourceOverview, ReviewQueuePage};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use serde::Serialize;
use time::OffsetDateTime;

/// A ListingSource as its own console page shows it.
#[derive(Debug, Serialize)]
pub struct ListingSourceDetail {
    pub listing_source: ListingSourceOverview,
    pub url_pattern: UrlPatternOverview,
    /// The live product schemas the scraper extracts with; `None` while the ListingSource has
    /// none stored.
    pub product_schema: Option<ProductSchemaOverview>,
    /// The first page of the ListingSource's reviews of both artifact types, open and closed;
    /// `GET /api/reviews?listing_source_id=` reads on.
    pub reviews: ReviewQueuePage,
}

/// The live URL pattern of a ListingSource: what the spider classifies product URLs by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UrlPatternOverview {
    /// `UNKNOWN` before a pattern was decided, `MATCHED` with one, `NO_PATTERN` when the shop has
    /// none.
    pub state: String,
    /// The regex a product URL matches; `None` unless `state` is `MATCHED`.
    pub pattern: Option<String>,
    /// The regex that takes a URL out of the pattern's matches; `None` without one.
    pub exclude: Option<String>,
}

/// The live product schemas of a ListingSource, as the scraper holds them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProductSchemaOverview {
    pub schemas: Vec<ProductCssSelectorSchema>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}
