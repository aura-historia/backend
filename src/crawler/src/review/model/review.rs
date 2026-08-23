use serde::{Deserialize, Serialize};
use shop_core::shop_id::ShopId;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReview {
    pub review_id: uuid::Uuid,
    pub shop_id: ShopId,
    pub shop_name: Option<String>,
    pub domain_id: Option<uuid::Uuid>,
    pub artifact_type: String,
    pub status: String,
    pub reason: String,
    pub candidate_payload: serde_json::Value,
    pub validation_summary: serde_json::Value,
    pub reviewer_notes: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed: Option<OffsetDateTime>,
}
