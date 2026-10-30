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
    /// Whether, in `agent` authoring mode, `agent_rounds` reached the cap, so the review waits for
    /// a human. Always `false` in the other modes, where a human decides every review.
    pub human_required: bool,
    /// When a human last changed or judged the review, which handed it back to the agents and from
    /// which `agent_rounds` count; `None` while no human acted on it.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub handed_back: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// One page of the agents' queue, oldest first, and the totals of the whole queue.
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
    /// Those that wait for a human (`human_required`); none outside `agent` mode.
    pub waiting_for_human: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn summary(handed_back: Option<OffsetDateTime>) -> PendingReviewSummary {
        let created = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        PendingReviewSummary {
            review_id: CrawlerReviewId::new(),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: "Antiques".to_owned(),
            domain_id: None,
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            reason: "manual_schema_required".to_owned(),
            candidate_version: 1,
            repair_request: None,
            agent_rounds: 0,
            human_required: false,
            handed_back,
            created,
            updated: created,
        }
    }

    /// `handed_back` is an RFC 3339 time or `null`, and a queue entry of a console that does not
    /// answer it yet reads as no hand-back, so a runner of a newer build orders it as before.
    #[test]
    fn should_answer_handed_back_as_rfc3339_or_null_and_read_a_missing_one_as_none() {
        let handed_back = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_001);
        let handed = summary(Some(handed_back));
        let never_handed = summary(None);

        let handed_json = serde_json::to_value(&handed).unwrap_or_else(|e| panic!("{e}"));
        let never_handed_json =
            serde_json::to_value(&never_handed).unwrap_or_else(|e| panic!("{e}"));
        let mut older_console = never_handed_json.clone();
        if let Some(fields) = older_console.as_object_mut() {
            fields.remove("handed_back");
        }

        assert_eq!(handed_json["handed_back"], json!("2024-10-05T00:00:00Z"));
        assert_eq!(never_handed_json["handed_back"], json!(null));
        let read = |value: serde_json::Value| {
            serde_json::from_value::<PendingReviewSummary>(value).unwrap_or_else(|e| panic!("{e}"))
        };
        assert_eq!(read(handed_json), handed);
        assert_eq!(read(older_console), never_handed);
    }
}
