use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewUrl {
    pub review_url_id: uuid::Uuid,
    pub review_id: uuid::Uuid,
    pub url: String,
    pub previous_class: Option<String>,
    pub current_pattern_match: Option<bool>,
    pub candidate_pattern_match: Option<bool>,
    pub candidate_class: Option<String>,
}
