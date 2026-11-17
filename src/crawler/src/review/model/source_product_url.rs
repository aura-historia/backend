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
    /// The identity the URL's listing was captured under, which its later scrapes keep; `null`
    /// before its first capture.
    #[serde(default)]
    pub captured_identity: Option<String>,
    /// The identity the selected schema derived from the page at its last capture; `null` when it
    /// derived none or before it was recorded.
    #[serde(default)]
    pub page_identity: Option<String>,
}
