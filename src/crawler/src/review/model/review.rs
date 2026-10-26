use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReview {
    pub review_id: CrawlerReviewId,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    pub domain_id: Option<CrawlerDomainId>,
    pub artifact_type: String,
    pub status: String,
    pub reason: String,
    pub candidate_payload: serde_json::Value,
    pub validation_summary: serde_json::Value,
    pub reviewer_notes: Option<String>,
    /// The latest needs-repair verdict. While the review is pending it waits for that repair.
    pub repair_request: Option<RepairRequest>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed: Option<OffsetDateTime>,
}

/// Why and since when a reviewer asked for a review's candidate to be repaired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairRequest {
    pub note: String,
    #[serde(with = "time::serde::rfc3339")]
    pub requested: OffsetDateTime,
}
