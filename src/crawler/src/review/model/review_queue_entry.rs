//! A row of the console's review queue (`GET /api/reviews`). The contract is in
//! `docs/crawler/operations.md` (review service).

use super::RepairRequest;
use crate::scraper::css_selector::product_schema_service::{
    SchemaLlmEvaluationConfidence, SchemaLlmEvaluationDecision,
};
use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A review as the console's queue lists it: what the queue shows and what tells the console that
/// the selected review changed, without the candidate and the validation summary, which
/// `GET /api/reviews/{id}` answers for the selected review. A queue poll therefore does not grow
/// with a candidate's schemas or the schema matrix stored with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewQueueEntry {
    pub review_id: CrawlerReviewId,
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: Option<String>,
    /// Whether the ListingSource is crawled; a pending review of a disabled one holds back no
    /// crawling or scraping.
    pub listing_source_crawl_enabled: bool,
    pub domain_id: Option<CrawlerDomainId>,
    pub artifact_type: String,
    pub status: String,
    pub reason: String,
    pub candidate_version: i64,
    /// The latest needs-repair verdict. While the review is pending it waits for that repair.
    pub repair_request: Option<RepairRequest>,
    /// The needs-repair verdicts the review agents gave the review since a human last changed or
    /// judged it.
    pub agent_rounds: u32,
    /// Whether, in `agent` authoring mode, `agent_rounds` reached the cap, so the review waits for
    /// a human. Always `false` in the other modes.
    pub human_required: bool,
    /// The outcome of the crawler's LLM evaluation of the generated schema the review holds;
    /// `None` when no LLM evaluated it.
    pub auto_schema_evaluation: Option<AutoSchemaEvaluationOutcome>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reviewed: Option<OffsetDateTime>,
}

/// The outcome of the crawler's LLM evaluation of a generated schema, as the review's validation
/// summary keeps it under `auto_schema_evaluation`, without the model's summary and risks, which
/// the review detail shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutoSchemaEvaluationOutcome {
    /// The model's own decision.
    pub decision: SchemaLlmEvaluationDecision,
    /// The model's confidence in its decision.
    pub confidence: SchemaLlmEvaluationConfidence,
    /// Whether the crawler approved the schema itself, on the model's high-confidence approval and
    /// a passed deterministic gate in `auto` mode.
    #[serde(default)]
    pub approved_by_llm: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(auto_schema_evaluation: Option<AutoSchemaEvaluationOutcome>) -> ReviewQueueEntry {
        let created = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        ReviewQueueEntry {
            review_id: CrawlerReviewId::new(),
            listing_source_id: ListingSourceId::new(),
            listing_source_name: Some("Antiques".to_owned()),
            listing_source_crawl_enabled: true,
            domain_id: None,
            artifact_type: "PRODUCT_SCHEMA".to_owned(),
            status: "PENDING_REVIEW".to_owned(),
            reason: "initial_schema_generation".to_owned(),
            candidate_version: 3,
            repair_request: None,
            agent_rounds: 0,
            human_required: false,
            auto_schema_evaluation,
            created,
            updated: created,
            reviewed: None,
        }
    }

    /// The queue row answers what the queue shows and leaves the candidate and the validation
    /// summary, whose schema matrix grows with the schemas and pages, to the review detail.
    #[test]
    fn should_answer_the_evaluation_outcome_without_candidate_or_validation_summary() {
        let json = serde_json::to_value(entry(Some(AutoSchemaEvaluationOutcome {
            decision: SchemaLlmEvaluationDecision::Approve,
            confidence: SchemaLlmEvaluationConfidence::High,
            approved_by_llm: true,
        })))
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            json["auto_schema_evaluation"],
            json!({ "decision": "APPROVE", "confidence": "HIGH", "approved_by_llm": true })
        );
        assert_eq!(json["created"], json!("2024-10-04T00:00:00Z"));
        assert_eq!(json["reviewed"], json!(null));
        for detail_only in ["candidate_payload", "validation_summary", "reviewer_notes"] {
            assert!(json.get(detail_only).is_none(), "{detail_only} is answered");
        }
    }

    #[test]
    fn should_answer_a_missing_evaluation_as_null() {
        let json = serde_json::to_value(entry(None)).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(json["auto_schema_evaluation"], json!(null));
    }

    /// The validation summary keeps the whole evaluation; the outcome reads only its decision,
    /// confidence and whether the crawler approved, and an evaluation stored before the crawler
    /// recorded its own approval reads as not approved by it.
    #[test]
    fn should_read_the_outcome_of_a_stored_evaluation() {
        let read = |stored: serde_json::Value| {
            serde_json::from_value::<AutoSchemaEvaluationOutcome>(stored)
                .unwrap_or_else(|error| panic!("{error}"))
        };

        assert_eq!(
            read(json!({
                "decision": "REJECT",
                "confidence": "MEDIUM",
                "approved_by_llm": false,
                "summary": "The title selector matches the shop name.",
                "risks": ["title"]
            })),
            AutoSchemaEvaluationOutcome {
                decision: SchemaLlmEvaluationDecision::Reject,
                confidence: SchemaLlmEvaluationConfidence::Medium,
                approved_by_llm: false,
            }
        );
        assert!(
            !read(json!({ "decision": "APPROVE", "confidence": "HIGH", "summary": "" }))
                .approved_by_llm
        );
    }
}
