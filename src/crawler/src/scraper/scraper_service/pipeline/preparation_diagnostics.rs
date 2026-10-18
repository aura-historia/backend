//! Records crawler candidates discarded during deterministic preparation.
//!
//! A discarded candidate never reaches raw capture, so its exact preparation input is snapshot
//! into the replay corpus before the scrape drops it. Recording failures are operational scrape
//! failures and use the existing URL retry handling.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};
use crate::scraper::normalization::preparation_snapshot::{
    CrawlerPreparationSnapshot, MAX_PREPARATION_SNAPSHOT_JSON_BYTES,
};
use crate::scraper::normalization::product_normalization_service::{
    ProductNormalizationContext, evaluate_preparation,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{NORMALIZATION_DIAGNOSTICS_VERSION, NormalizationIssue};
use product_listing_service::normalization_diagnostics::{
    BUILD_GIT_SHA, NormalizationUnsolvedEventContext, log_normalization_unsolved,
};
use product_listing_service::ports::{
    NormalizationDiagnosticCase, NormalizationDiagnosticEntryPoint,
    NormalizationDiagnosticEvidence, NormalizationDiagnosticOutcome,
    NormalizationDiagnosticProvenance,
};
use tracing::warn;

impl ScraperServiceImpl {
    /// Records one discarded candidate. `error` is the decision that discarded it; the pure
    /// evaluator supplies every independent issue for the same input.
    pub(crate) async fn record_discarded_candidate(
        &self,
        listing_source_id: &ListingSourceId,
        prepared_input: &RawExtractedProduct,
        context: &ProductNormalizationContext,
        error: &NormalizationError,
    ) -> Result<(), ScraperError> {
        let mut issues = evaluate_preparation(prepared_input.clone(), context.clone())
            .report
            .into_issues();
        let decision = NormalizationIssue::from_normalization_error(error);
        if !issues.contains(&decision) {
            issues.push(decision);
        }
        let outcome = match error.failure_scope() {
            NormalizationFailureScope::CandidateData => NormalizationDiagnosticOutcome::Discarded,
            NormalizationFailureScope::System => NormalizationDiagnosticOutcome::SystemFailure,
        };

        let snapshot = CrawlerPreparationSnapshot::new(prepared_input, context);
        let input_sha256 = snapshot
            .input_sha256()
            .map_err(ScraperError::RawNormalizationInput)?;
        let snapshot = snapshot.to_json();
        let retainable = serde_json::to_vec(&snapshot)
            .is_ok_and(|encoded| encoded.len() <= MAX_PREPARATION_SNAPSHOT_JSON_BYTES);
        let case = NormalizationDiagnosticCase {
            entry_point: NormalizationDiagnosticEntryPoint::CrawlerPreparationV1,
            input_sha256,
            listing_source_id: *listing_source_id,
            evidence: NormalizationDiagnosticEvidence::Snapshot(snapshot),
            outcome,
            issues: issues.clone(),
            provenance: NormalizationDiagnosticProvenance {
                diagnostics_version: NORMALIZATION_DIAGNOSTICS_VERSION,
                evaluator_version: CRAWLER_PREPARATION_EVALUATOR_VERSION,
                build: BUILD_GIT_SHA.to_owned(),
            },
        };
        let case_id = case.case_id();
        let evidence_recorded = match (&self.normalization_diagnostics, retainable) {
            (Some(recorder), true) => {
                recorder
                    .record(case)
                    .await
                    .map_err(ScraperError::NormalizationDiagnosticCaseRecordFailed)?;
                true
            }
            (Some(_), false) => {
                warn!(
                    %case_id,
                    max_bytes = MAX_PREPARATION_SNAPSHOT_JSON_BYTES,
                    "crawler preparation snapshot exceeds the evidence limit and is not retained"
                );
                false
            }
            (None, _) => false,
        };
        log_normalization_unsolved(
            &NormalizationUnsolvedEventContext {
                case_id,
                input_sha256: &input_sha256,
                entry_point: NormalizationDiagnosticEntryPoint::CrawlerPreparationV1,
                outcome,
                evaluator_version: CRAWLER_PREPARATION_EVALUATOR_VERSION,
                listing_source_id: Some(listing_source_id.to_string()),
                product_listing_raw_stream_id: None,
                product_listing_raw_revision_id: None,
                revision: None,
                request_id: None,
                evidence_recorded,
            },
            &issues,
        );
        Ok(())
    }
}

/// Provenance version of crawler preparation heuristics recorded with each case.
pub(crate) const CRAWLER_PREPARATION_EVALUATOR_VERSION: u16 = 1;
