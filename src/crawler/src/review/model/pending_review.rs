//! The review agents' queue (`GET /api/pending-reviews`): the pending reviews of crawl-enabled
//! ListingSources, read page by page, with the totals of the whole queue. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::RepairRequest;
use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A pending review of a crawl-enabled ListingSource as the agents' queue lists it: what a review
/// is picked by, without its candidate and validation summary, which `GET /api/reviews/{id}`
/// answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReviewSummary {
    pub review_id: CrawlerReviewId,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub domain_id: Option<CrawlerDomainId>,
    pub artifact_type: String,
    pub reason: String,
    pub candidate_version: i64,
    /// The latest needs-repair verdict, which the review waits for.
    pub repair_request: Option<RepairRequest>,
    /// The needs-repair verdicts the review agents gave the review since a human last changed or
    /// judged it.
    pub agent_rounds: u32,
    /// Whether `agent_rounds` reached the cap, so the review waits for a human.
    pub human_required: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// One page of the agents' queue, newest first, and the totals of the whole queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReviewPage {
    pub reviews: Vec<PendingReviewSummary>,
    /// The review to read on after, as `after`, when reviews follow this page.
    pub next_after: Option<CrawlerReviewId>,
    pub totals: PendingReviewTotals,
}

/// How many pending reviews of crawl-enabled ListingSources there are, whatever page or filter was
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingReviewTotals {
    /// Those the review agents can work.
    pub workable: i64,
    /// Those that wait for a human (`human_required`).
    pub waiting_for_human: i64,
}
