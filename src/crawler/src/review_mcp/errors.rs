//! A review tool's failure as the agent sees it: a tool error whose text is one JSON object with a
//! `kind`, the `error` message and, when the console refused the request, its `status` and the
//! console's own `reason`, `failed_checks`, `candidate_version`, `coverage`, `retry_after_secs`,
//! `limit`, `agent_rounds` and `details`, passed through as the console answered them. Nothing in
//! it comes from a request or response header, so no token or cookie can reach the agent.

use super::bounds::{MAX_TOOL_OUTPUT_BYTES, truncated_text};
use super::console::{ConsoleError, ConsoleRefusal};
use crate::CrawlerReviewId;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::{Map, Value};

/// The console keys of a refusal an agent acts on, passed through as the console answered them.
const PASSED_THROUGH: [&str; 8] = [
    "reason",
    "failed_checks",
    "candidate_version",
    "coverage",
    "retry_after_secs",
    "limit",
    "agent_rounds",
    "details",
];
/// The longest message a tool error carries, in characters.
const MAX_ERROR_CHARS: usize = 2_000;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error(
        "URL `{0}` is not a plain http(s) URL on the default port, without credentials, on a domain of the review's ListingSource; the review tools fetch no other URL"
    )]
    UrlOutsideReviewSource(String),
    #[error(
        "this agent process works review {0} only; the review tools refuse every call that names no review or another one"
    )]
    ReviewOutsideScope(CrawlerReviewId),
    #[error("{0}")]
    Console(#[from] ConsoleError),
    #[error(
        "the {0} is still running after the server's wait; call the tool again later to wait for it again"
    )]
    StillRunning(&'static str),
    #[error(
        "the review's stored candidate does not decode as a {0} candidate; leave the review to a human"
    )]
    UndecodableCandidate(&'static str),
    #[error("{0}")]
    NotApplicable(String),
    #[error("the answer is too large to return even after cutting it; leave the review to a human")]
    AnswerTooLarge,
}

#[derive(Serialize)]
struct ErrorReport {
    kind: &'static str,
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(flatten)]
    console: Map<String, Value>,
}

impl ToolError {
    /// The tool result that reports this failure to the agent.
    pub fn into_tool_result(self) -> CallToolResult {
        let report = self.report();
        let text = serde_json::to_string(&report).unwrap_or_else(|_| {
            format!(
                "{{\"kind\":\"{}\",\"error\":\"internal error\"}}",
                report.kind
            )
        });
        let text = if text.len() > MAX_TOOL_OUTPUT_BYTES {
            // Only passed-through console evidence can grow this large; the message stays.
            serde_json::to_string(&ErrorReport {
                console: Map::new(),
                ..report
            })
            .unwrap_or_default()
        } else {
            text
        };
        CallToolResult::error(vec![ContentBlock::text(text)])
    }

    /// The kind an agent tells failures apart by.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidArguments(_) => "INVALID_ARGUMENTS",
            Self::UrlOutsideReviewSource(_) => "URL_OUTSIDE_REVIEW_SOURCE",
            Self::ReviewOutsideScope(_) => "REVIEW_OUTSIDE_SCOPE",
            Self::Console(ConsoleError::Refused(refusal)) => refusal_kind(refusal.status),
            Self::Console(ConsoleError::Unauthorized | ConsoleError::TokenFile(_)) => {
                "UNAUTHORIZED"
            }
            Self::Console(_) => "CONSOLE_UNAVAILABLE",
            Self::StillRunning(_) => "STILL_RUNNING",
            Self::UndecodableCandidate(_) => "UNDECODABLE_CANDIDATE",
            Self::NotApplicable(_) => "NOT_APPLICABLE",
            Self::AnswerTooLarge => "ANSWER_TOO_LARGE",
        }
    }

    fn report(&self) -> ErrorReport {
        let kind = self.kind();
        match self {
            Self::Console(ConsoleError::Refused(refusal)) => refusal_report(kind, refusal),
            Self::Console(ConsoleError::Unauthorized | ConsoleError::TokenFile(_)) => ErrorReport {
                kind,
                error: format!(
                    "{self}; the crawler may have restarted outside agent authoring mode. Stop and report it"
                ),
                status: Some(401),
                console: Map::new(),
            },
            _ => ErrorReport {
                kind,
                error: truncated_text(&self.to_string(), MAX_ERROR_CHARS),
                status: None,
                console: Map::new(),
            },
        }
    }
}

fn refusal_kind(status: u16) -> &'static str {
    match status {
        400 => "INVALID_REQUEST",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        409 => "CONFLICT",
        429 => "RATE_LIMITED",
        500..=599 => "CONSOLE_FAILED",
        _ => "REFUSED",
    }
}

fn refusal_report(kind: &'static str, refusal: &ConsoleRefusal) -> ErrorReport {
    let message = refusal
        .body
        .get("error")
        .and_then(Value::as_str)
        .map_or_else(
            || {
                format!(
                    "the review console refused the request with status {}",
                    refusal.status
                )
            },
            |error| truncated_text(error, MAX_ERROR_CHARS),
        );
    let console = PASSED_THROUGH
        .iter()
        .filter_map(|key| {
            refusal
                .body
                .get(*key)
                .filter(|value| !value.is_null())
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect();
    ErrorReport {
        kind,
        error: message,
        status: Some(refusal.status),
        console,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reported(error: ToolError) -> Value {
        let result = error.into_tool_result();
        assert_eq!(result.is_error, Some(true));
        assert!(result.structured_content.is_none());
        let text = result
            .content
            .first()
            .and_then(|content| content.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}: {text}"))
    }

    fn refused(status: u16, body: Value) -> ToolError {
        ToolError::Console(ConsoleError::Refused(ConsoleRefusal { status, body }))
    }

    #[test]
    fn should_pass_the_console_refusal_through_with_its_evidence() {
        let guard = reported(refused(
            409,
            json!({
                "error": "the review agent's approval fails the approval guard",
                "failed_checks": [{ "check": "TOO_FEW_LABELS", "product": 1 }],
                "internal": "not passed",
            }),
        ));
        let role = reported(refused(
            403,
            json!({ "error": "the author agent neither approves nor rejects", "reason": "AUTHOR_CANNOT_DECIDE" }),
        ));
        let stale = reported(refused(
            409,
            json!({ "error": "review changed", "candidate_version": 7 }),
        ));
        let cap = reported(refused(
            429,
            json!({ "error": "cap", "reason": "AGENT_DECISION_LIMIT", "limit": 20, "retry_after_secs": 600 }),
        ));
        let waiting = reported(refused(
            409,
            json!({ "error": "waits for a human", "reason": "HUMAN_REQUIRED", "agent_rounds": 3, "limit": 3 }),
        ));
        let invalid = reported(refused(400, json!({ "error": "invalid review page url" })));

        assert_eq!(
            guard,
            json!({
                "kind": "CONFLICT",
                "error": "the review agent's approval fails the approval guard",
                "status": 409,
                "failed_checks": [{ "check": "TOO_FEW_LABELS", "product": 1 }],
            })
        );
        assert_eq!(role["kind"], "FORBIDDEN");
        assert_eq!(role["reason"], "AUTHOR_CANNOT_DECIDE");
        assert_eq!(stale["candidate_version"], 7);
        assert_eq!(
            (
                cap["kind"].clone(),
                cap["limit"].clone(),
                cap["retry_after_secs"].clone()
            ),
            (json!("RATE_LIMITED"), json!(20), json!(600))
        );
        assert_eq!(
            waiting,
            json!({
                "kind": "CONFLICT",
                "error": "waits for a human",
                "status": 409,
                "reason": "HUMAN_REQUIRED",
                "agent_rounds": 3,
                "limit": 3,
            })
        );
        assert_eq!(invalid["kind"], "INVALID_REQUEST");
        assert_eq!(invalid["status"], 400);
    }

    #[test]
    fn should_report_its_own_failures_by_kind() {
        for (error, kind) in [
            (
                ToolError::InvalidArguments("missing field `review_id`".to_owned()),
                "INVALID_ARGUMENTS",
            ),
            (
                ToolError::UrlOutsideReviewSource("https://elsewhere.example/".to_owned()),
                "URL_OUTSIDE_REVIEW_SOURCE",
            ),
            (
                ToolError::Console(ConsoleError::Unauthorized),
                "UNAUTHORIZED",
            ),
            (
                ToolError::Console(ConsoleError::AnswerTooLarge),
                "CONSOLE_UNAVAILABLE",
            ),
            (ToolError::StillRunning("matrix refresh"), "STILL_RUNNING"),
            (
                refused(
                    502,
                    json!({ "error": "failed to fetch", "details": "timeout" }),
                ),
                "CONSOLE_FAILED",
            ),
        ] {
            let report = reported(error);
            assert_eq!(report["kind"], kind, "{report}");
            assert!(
                report["error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty())
            );
        }
    }

    #[test]
    fn should_drop_oversized_console_evidence_but_keep_the_message() {
        let report = reported(refused(
            409,
            json!({ "error": "guard", "failed_checks": vec!["x".repeat(1_000); 100] }),
        ));

        assert_eq!(
            report,
            json!({ "kind": "CONFLICT", "error": "guard", "status": 409 })
        );
    }
}
