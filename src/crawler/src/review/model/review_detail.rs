use super::{CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ReviewDetail {
    pub review: CrawlerReview,
    /// Whether the review is an approved product-schema review whose candidate is the
    /// ListingSource's live schema list: edits through it change the live list, and edits through
    /// any other approved review are refused.
    pub candidate_is_live: bool,
    pub pages: Vec<CrawlerReviewPage>,
    pub urls: Vec<CrawlerReviewUrl>,
}
