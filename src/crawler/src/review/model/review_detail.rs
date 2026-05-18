use super::{CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ReviewDetail {
    pub review: CrawlerReview,
    pub pages: Vec<CrawlerReviewPage>,
    pub urls: Vec<CrawlerReviewUrl>,
}
