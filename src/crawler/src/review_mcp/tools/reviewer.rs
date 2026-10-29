//! The reviewer agent's tools: approving and rejecting what the author agent or a human staged,
//! within the console's approval guard.

use super::{Effect, ToolContext, ToolDefinition, ToolSchemaError, review_path};
use crate::CrawlerReviewId;
use crate::review_mcp::errors::ToolError;
use crate::review_mcp::views::VerdictTaken;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

const APPROVE: &str = "Approve a pending review's candidate at expected_candidate_version, the version you checked: the console makes it live for the shop. Approve only what you verified on every page yourself: for product schemas, the matrix of that version, refreshed with refresh_matrix after the last change, shows every review page covered with plausible values; for a URL pattern, you opened the labelled pages and agree with every label, and the pattern takes every PRODUCT URL and no NOT_PRODUCT URL. Never approve on a guess. The console decides with its approval guard: CONFLICT with failed_checks names each check that failed (such as LIVE_MATRIX_MISSING, LIVE_MATRIX_OUTDATED, LIVE_MATRIX_EXPIRED, PAGES_NOT_EVALUATED, PAGES_NOT_COVERED, REQUIRED_FIELD_EMPTY, RULE_MATCHES_NOTHING, IMPLAUSIBLE_EXTRACTION, CANDIDATE_UNDECIDED, TOO_FEW_LABELS, PRODUCT_LABEL_NOT_MATCHED, NOT_PRODUCT_LABEL_MATCHED, APPROVER_IS_AUTHOR): report them, do not argue with them; CONFLICT with candidate_version means the candidate changed since you read it; FORBIDDEN with a reason means agents may not decide now (AGENT_DECISIONS_OFF) or not this way; RATE_LIMITED means the agents made their decisions of the hour: stop and retry after retry_after_secs; INVALID_REQUEST means the review is no longer pending. notes are kept with the approval.";

const REJECT: &str = "Reject a pending review: its candidate is discarded, nothing goes live, and the work waiting on the review is released. Reject only a candidate that cannot work; when it can be fixed, use mark_needs_repair instead. Rejecting a manual_schema_required review also classifies its seed and triggering pages, not its waiting or reviewer-added ones, as no product pages: the crawler no longer captures them as products, not even once another review approves a schema that extracts them, and opens no new review for them. So reject such a review only when those pages are no product pages. notes are required and must say why. FORBIDDEN with a reason means agents may not decide now; RATE_LIMITED means the agents made their decisions of the hour: retry after retry_after_secs.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ApproveArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// The candidate_version you checked.
    expected_candidate_version: i64,
    /// Why the candidate is right, kept with the approval.
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct RejectArguments {
    /// The review's id, such as `cr_01...`, as list_pending_reviews names it.
    #[schemars(with = "String")]
    review_id: CrawlerReviewId,
    /// Why the candidate cannot work.
    notes: String,
}

pub(super) fn tools() -> Result<Vec<ToolDefinition>, ToolSchemaError> {
    Ok(vec![
        ToolDefinition::new("approve", APPROVE, Effect::Decides, approve)?,
        ToolDefinition::new("reject", REJECT, Effect::Decides, reject)?,
    ])
}

async fn approve(
    arguments: ApproveArguments,
    context: Arc<ToolContext>,
) -> Result<VerdictTaken, ToolError> {
    let mut approval = Map::new();
    approval.insert(
        "expected_candidate_version".to_owned(),
        json!(arguments.expected_candidate_version),
    );
    if let Some(notes) = arguments
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
    {
        approval.insert("notes".to_owned(), json!(notes));
    }
    context
        .console
        .post(
            &review_path(arguments.review_id, "/approve"),
            &Value::Object(approval),
        )
        .await?;
    Ok(VerdictTaken {
        review_id: arguments.review_id.to_string(),
        verdict: "APPROVED".to_owned(),
    })
}

async fn reject(
    arguments: RejectArguments,
    context: Arc<ToolContext>,
) -> Result<VerdictTaken, ToolError> {
    context
        .console
        .post(
            &review_path(arguments.review_id, "/reject"),
            &json!({ "notes": arguments.notes }),
        )
        .await?;
    Ok(VerdictTaken {
        review_id: arguments.review_id.to_string(),
        verdict: "REJECTED".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_never_take_a_coverage_acknowledgement_with_an_approval() {
        let review_id = CrawlerReviewId::new().to_string();

        assert!(
            serde_json::from_value::<ApproveArguments>(json!({
                "review_id": review_id,
                "expected_candidate_version": 4,
            }))
            .is_ok()
        );
        for arguments in [
            json!({ "review_id": review_id }),
            json!({
                "review_id": review_id,
                "expected_candidate_version": 4,
                "acknowledged_coverage": { "covered": 1, "removed": 0, "total": 2, "not_evaluated": 0 },
            }),
        ] {
            assert!(
                serde_json::from_value::<ApproveArguments>(arguments.clone()).is_err(),
                "{arguments}"
            );
        }
    }
}
