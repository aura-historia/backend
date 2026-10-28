//! The reports the agents end their process with, as the role's report schema
//! (`schemas/*-report.v1.json`) asks for them. A report is the agent's own account and decides
//! nothing: the console's event trail is what happened. The runner checks that a report is the
//! role's report for the review it was given, keeps it bounded in its ledger and log, and reads
//! the reviewer's decision and the server's answer to schedule the review's next pass.

use crate::CrawlerReviewId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The longest summary, risk, finding or detail text a recorded report keeps, in characters.
const MAX_TEXT_CHARS: usize = 2_000;
/// The most risks, findings or failed checks a recorded report keeps.
const MAX_ITEMS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthorAction {
    /// The author staged a candidate for the reviewer.
    Staged,
    /// The current candidate already holds, or no candidate can work.
    NothingToStage,
    /// The author could not work the review.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorReport {
    pub review_id: String,
    pub action: AuthorAction,
    pub candidate_version: i64,
    pub summary: String,
    pub risks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewerDecision {
    Approve,
    Reject,
    NeedsRepair,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerOutcome {
    Accepted,
    Refused,
    NotSent,
}

/// What the console answered the reviewer's decision, as the reviewer reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerAnswer {
    pub outcome: ServerOutcome,
    pub kind: String,
    pub failed_checks: Vec<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldFinding {
    pub schema_index: i64,
    pub field: String,
    pub selector: String,
    pub finding: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewerReport {
    pub review_id: String,
    pub decision: ReviewerDecision,
    pub confidence: Confidence,
    pub summary: String,
    pub risks: Vec<String>,
    pub field_findings: Vec<FieldFinding>,
    pub server_answer: ServerAnswer,
}

impl ReviewerReport {
    /// Whether the console took a decision that closes the review or hands it back to the author
    /// with notes: anything else leaves the review where it was.
    pub fn decided(&self) -> bool {
        self.server_answer.outcome == ServerOutcome::Accepted
            && matches!(
                self.decision,
                ReviewerDecision::Approve
                    | ReviewerDecision::Reject
                    | ReviewerDecision::NeedsRepair
            )
    }
}

/// A role's report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum AgentReport {
    Author(AuthorReport),
    Reviewer(ReviewerReport),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReportError {
    #[error("the report is not the role's report: {0}")]
    Malformed(String),
    #[error("the report names another review than the one the agent was given")]
    OtherReview,
}

impl AgentReport {
    /// The author's `structured_output` for `review_id`, bounded.
    pub fn author(output: Value, review_id: CrawlerReviewId) -> Result<Self, ReportError> {
        let report: AuthorReport = decode(output)?;
        same_review(&report.review_id, review_id)?;
        Ok(Self::Author(AuthorReport {
            summary: bounded(&report.summary),
            risks: bounded_list(&report.risks),
            ..report
        }))
    }

    /// The reviewer's `structured_output` for `review_id`, bounded.
    pub fn reviewer(output: Value, review_id: CrawlerReviewId) -> Result<Self, ReportError> {
        let report: ReviewerReport = decode(output)?;
        same_review(&report.review_id, review_id)?;
        Ok(Self::Reviewer(ReviewerReport {
            summary: bounded(&report.summary),
            risks: bounded_list(&report.risks),
            field_findings: report
                .field_findings
                .iter()
                .take(MAX_ITEMS)
                .map(|finding| FieldFinding {
                    schema_index: finding.schema_index,
                    field: bounded(&finding.field),
                    selector: bounded(&finding.selector),
                    finding: bounded(&finding.finding),
                })
                .collect(),
            server_answer: ServerAnswer {
                kind: bounded(&report.server_answer.kind),
                failed_checks: bounded_list(&report.server_answer.failed_checks),
                detail: bounded(&report.server_answer.detail),
                ..report.server_answer.clone()
            },
            ..report
        }))
    }
}

fn decode<T: serde::de::DeserializeOwned>(output: Value) -> Result<T, ReportError> {
    serde_json::from_value(output).map_err(|error| ReportError::Malformed(error.to_string()))
}

fn same_review(reported: &str, review_id: CrawlerReviewId) -> Result<(), ReportError> {
    if reported.trim() == review_id.to_string() {
        Ok(())
    } else {
        Err(ReportError::OtherReview)
    }
}

fn bounded(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().take(MAX_TEXT_CHARS).collect();
    format!("{kept}… [truncated]")
}

fn bounded_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .take(MAX_ITEMS)
        .map(|item| bounded(item))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reviewer_output(review_id: &str, decision: &str, outcome: &str) -> Value {
        json!({
            "review_id": review_id,
            "decision": decision,
            "confidence": "HIGH",
            "summary": "Checked /item/1 and /item/2.",
            "risks": [],
            "field_findings": [
                { "schema_index": 0, "field": "price", "selector": ".price", "finding": "empty on /item/2" }
            ],
            "server_answer": {
                "outcome": outcome,
                "kind": if outcome == "REFUSED" { "CONFLICT" } else { "" },
                "failed_checks": if outcome == "REFUSED" { json!(["TOO_FEW_LABELS"]) } else { json!([]) },
                "detail": "",
            },
        })
    }

    #[test]
    fn should_read_each_role_s_report_for_its_review() {
        let review_id = CrawlerReviewId::new();
        let author = AgentReport::author(
            json!({
                "review_id": review_id.to_string(),
                "action": "STAGED",
                "candidate_version": 4,
                "summary": "Staged two schemas.",
                "risks": ["POA pages untested"],
            }),
            review_id,
        );
        let reviewer = AgentReport::reviewer(
            reviewer_output(&review_id.to_string(), "NEEDS_REPAIR", "ACCEPTED"),
            review_id,
        );

        assert!(matches!(
            author,
            Ok(AgentReport::Author(AuthorReport {
                action: AuthorAction::Staged,
                candidate_version: 4,
                ..
            }))
        ));
        match reviewer {
            Ok(AgentReport::Reviewer(report)) => {
                assert_eq!(report.decision, ReviewerDecision::NeedsRepair);
                assert!(report.decided());
                assert_eq!(report.field_findings.len(), 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn should_refuse_a_report_for_another_review_or_of_another_shape() {
        let review_id = CrawlerReviewId::new();

        assert_eq!(
            AgentReport::reviewer(
                reviewer_output(&CrawlerReviewId::new().to_string(), "APPROVE", "ACCEPTED"),
                review_id,
            ),
            Err(ReportError::OtherReview)
        );
        for output in [
            reviewer_output(&review_id.to_string(), "MAYBE", "ACCEPTED"),
            json!({ "review_id": review_id.to_string(), "action": "STAGED" }),
            json!("approve"),
        ] {
            assert!(
                matches!(
                    AgentReport::reviewer(output.clone(), review_id),
                    Err(ReportError::Malformed(_))
                ),
                "{output}"
            );
        }
        let mut extra = reviewer_output(&review_id.to_string(), "APPROVE", "ACCEPTED");
        extra["token"] = json!("x");
        assert!(matches!(
            AgentReport::reviewer(extra, review_id),
            Err(ReportError::Malformed(_))
        ));
    }

    #[test]
    fn should_count_only_a_decision_the_server_took() {
        let review_id = CrawlerReviewId::new();
        let decided = |decision: &str, outcome: &str| match AgentReport::reviewer(
            reviewer_output(&review_id.to_string(), decision, outcome),
            review_id,
        ) {
            Ok(AgentReport::Reviewer(report)) => report.decided(),
            other => panic!("{other:?}"),
        };

        assert!(decided("APPROVE", "ACCEPTED"));
        assert!(decided("REJECT", "ACCEPTED"));
        assert!(!decided("APPROVE", "REFUSED"));
        assert!(!decided("SKIP", "NOT_SENT"));
    }

    #[test]
    fn should_bound_the_texts_it_keeps() {
        let review_id = CrawlerReviewId::new();
        let report = AgentReport::author(
            json!({
                "review_id": review_id.to_string(),
                "action": "SKIPPED",
                "candidate_version": 1,
                "summary": "x".repeat(10_000),
                "risks": vec!["r"; 100],
            }),
            review_id,
        );

        match report {
            Ok(AgentReport::Author(report)) => {
                assert!(report.summary.chars().count() < 2_100);
                assert_eq!(report.risks.len(), 30);
            }
            other => panic!("{other:?}"),
        }
    }
}
