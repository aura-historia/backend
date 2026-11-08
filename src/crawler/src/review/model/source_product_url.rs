use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A product URL the crawler knows of the review's ListingSource, offered to a product-schema
/// review's author as a page to test the candidate on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProductUrl {
    pub url: String,
    /// When the scraper last evaluated the page; `null` before its first scrape.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_scraped: Option<OffsetDateTime>,
    /// The error kind of the last scrape, such as `MissingRequiredValue`; `null` when it scraped
    /// without one.
    pub last_error_kind: Option<String>,
}
