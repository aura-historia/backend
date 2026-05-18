use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewPage {
    pub review_page_id: uuid::Uuid,
    pub review_id: uuid::Uuid,
    pub url: String,
    pub role: String,
    pub raw_html: String,
    pub cleaned_html: String,
    pub html_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched: OffsetDateTime,
}
