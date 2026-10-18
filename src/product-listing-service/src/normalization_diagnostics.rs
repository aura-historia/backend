//! Structured `product_listing_normalization_unsolved` CloudWatch events.
//!
//! The owning entry-point boundary emits one event per issue after deciding how the caller
//! handled the evaluation. Events reference the replay case and raw evidence; they never contain
//! input text, provider payloads or credentials.

use crate::ports::{
    NormalizationDiagnosticCaseId, NormalizationDiagnosticEntryPoint,
    NormalizationDiagnosticOutcome,
};
use product_listing_normalization::{
    NORMALIZATION_DIAGNOSTICS_VERSION, NormalizationIssue, NormalizationIssueScope,
};

pub const NORMALIZATION_UNSOLVED_EVENT: &str = "product_listing_normalization_unsolved";
pub const NORMALIZATION_UNSOLVED_EVENT_VERSION: u16 = 1;

/// Git SHA of the deployed build, stamped at compile time by the deploy workflow.
pub const BUILD_GIT_SHA: &str = match option_env!("DEPLOY_COMMIT_SHA") {
    Some(sha) => sha,
    None => match option_env!("GITHUB_SHA") {
        Some(sha) => sha,
        None => "unknown",
    },
};

/// References shared by every event for one evaluated input.
#[derive(Debug, Clone)]
pub struct NormalizationUnsolvedEventContext<'a> {
    pub case_id: NormalizationDiagnosticCaseId,
    pub input_sha256: &'a [u8],
    pub entry_point: NormalizationDiagnosticEntryPoint,
    pub outcome: NormalizationDiagnosticOutcome,
    pub evaluator_version: u16,
    pub listing_source_id: Option<String>,
    pub product_listing_raw_stream_id: Option<String>,
    pub product_listing_raw_revision_id: Option<String>,
    pub revision: Option<u64>,
    /// Crawler run or request correlation when available.
    pub request_id: Option<String>,
    /// Whether diagnostic case evidence was durably recorded for replay.
    pub evidence_recorded: bool,
}

/// Emits one event per issue. Candidate-data gaps log at WARN, system failures at ERROR.
pub fn log_normalization_unsolved(
    context: &NormalizationUnsolvedEventContext<'_>,
    issues: &[NormalizationIssue],
) {
    let input_sha256 = hex(context.input_sha256);
    for issue in issues {
        let details = serde_json::to_string(&issue.details).unwrap_or_default();
        macro_rules! emit {
            ($level:ident) => {
                tracing::$level!(
                    event = NORMALIZATION_UNSOLVED_EVENT,
                    event_version = NORMALIZATION_UNSOLVED_EVENT_VERSION,
                    case_id = %context.case_id,
                    input_sha256 = %input_sha256,
                    entry_point = context.entry_point.as_str(),
                    stage = issue.stage.as_str(),
                    field = issue.field.as_str(),
                    path = issue.field.path(),
                    reason = issue.reason.as_str(),
                    scope = issue.scope.as_str(),
                    handling = issue.handling.as_str(),
                    details = %details,
                    outcome = context.outcome.as_str(),
                    listing_source_id = context.listing_source_id.as_deref(),
                    product_listing_raw_stream_id =
                        context.product_listing_raw_stream_id.as_deref(),
                    product_listing_raw_revision_id =
                        context.product_listing_raw_revision_id.as_deref(),
                    revision = context.revision,
                    request_id = context.request_id.as_deref(),
                    evidence_recorded = context.evidence_recorded,
                    diagnostics_version = NORMALIZATION_DIAGNOSTICS_VERSION,
                    evaluator_version = context.evaluator_version,
                    git_sha = BUILD_GIT_SHA,
                    "product listing normalization left an input unsolved"
                )
            };
        }
        match issue.scope {
            NormalizationIssueScope::System => emit!(error),
            NormalizationIssueScope::CandidateData => emit!(warn),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_render_as_lowercase_hex() {
        assert_eq!("00ff10", hex(&[0, 255, 16]));
    }
}
