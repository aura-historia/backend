use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewPage {
    pub review_page_id: CrawlerReviewPageId,
    pub review_id: CrawlerReviewId,
    pub url: String,
    pub role: String,
    pub html_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched: OffsetDateTime,
    /// Why the author or a human marked the page as no product page, `None` for a page the
    /// candidate has to resolve. A marked page is left out of the schema matrix and the coverage.
    pub not_product_reason: Option<String>,
}

impl CrawlerReviewPage {
    pub fn is_marked_not_product(&self) -> bool {
        self.not_product_reason.is_some()
    }
}
