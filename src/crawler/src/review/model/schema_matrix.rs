use super::{
    CrawlerReviewPage, LinkIdentity, PageSignals, RemovedPageTemplateMatch,
    SchemaCandidateEvaluation,
};
use crate::review::captured_identity_sample::CapturedIdentitySample;
use crate::review::page_coverage::{PageCoverage, page_coverages};
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMatrix {
    pub review_id: Option<CrawlerReviewId>,
    /// The name of the review's ListingSource, which the cross-page checks look for in titles;
    /// `None` for a matrix of the pages schema generation was given and one stored before the
    /// check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_source_name: Option<String>,
    pub candidates: Vec<SchemaCandidateEvaluation>,
    /// The review pages the candidate's removed-page templates match; empty without templates.
    #[serde(default)]
    pub removed_page_matches: Vec<RemovedPageTemplateMatch>,
    /// What each evaluated page shows that a schema should capture; empty for a matrix stored
    /// before the signals, which the approval guard treats as outdated.
    #[serde(default)]
    pub page_signals: Vec<PageSignals>,
    /// What the server evaluated when it evaluated this matrix on the review's live pages;
    /// `None` for a matrix of the pages schema generation was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_evaluation: Option<LiveMatrixEvaluation>,
}

impl SchemaMatrix {
    /// The schema matrix a review's validation summary stores, `None` when it stores none, or the
    /// decoding error of one that no longer decodes, such as a matrix stored before a later check.
    pub fn stored_in(
        validation_summary: &serde_json::Value,
    ) -> Option<Result<Self, serde_json::Error>> {
        let stored = validation_summary.get("schema_matrix")?;
        Some(serde_json::from_value(stored.clone()))
    }
}

/// What a live schema matrix evaluated: the review's candidate at `candidate_version`, as the
/// product-schema configuration that approving it would have made live, on the review pages as
/// the shop served them from `fetched_at` on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveMatrixEvaluation {
    pub candidate_version: i64,
    /// SHA-256 of the schema list and removed-page templates the matrix evaluated, so an approval
    /// can tell whether it would make exactly those live.
    pub configuration_sha256: String,
    /// When the first review page was fetched.
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
    /// What the schemas derive from a sample of the ListingSource's captured product URLs
    /// outside the review pages; `None` for a matrix stored before the sample, which the approval
    /// guard treats as outdated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_identity_sample: Option<CapturedIdentitySample>,
}

impl LiveMatrixEvaluation {
    /// Whether an identity that is a URL or path keeps the evaluated candidate out of an agent's
    /// approval: it does unless the captured-identity sample shows the ListingSource's listings
    /// captured under such identities, which the candidate keeps deriving. So a URL identity stays
    /// approvable where it is already the listings' identity, and no source starts using one.
    pub(crate) fn link_identity(&self) -> LinkIdentity {
        match &self.captured_identity_sample {
            Some(sample) if sample.keeps_captured_link_identities() => LinkIdentity::Captured,
            _ => LinkIdentity::Blocks,
        }
    }
}

/// A review's schema matrix as the review API answers it: the matrix, and the review pages it
/// did not evaluate, which count as not covered until the next evaluation.
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSchemaMatrix {
    #[serde(flatten)]
    pub matrix: SchemaMatrix,
    pub not_evaluated_pages: Vec<CrawlerReviewPageId>,
}

impl ReviewSchemaMatrix {
    /// `matrix` with the review's current `pages` it did not evaluate.
    pub fn of(matrix: SchemaMatrix, pages: &[CrawlerReviewPage]) -> Self {
        let not_evaluated_pages = page_coverages(Some(&matrix), pages)
            .into_iter()
            .filter(|(_, coverage)| *coverage == PageCoverage::NotEvaluated)
            .map(|(page, _)| page.review_page_id)
            .collect();
        Self {
            matrix,
            not_evaluated_pages,
        }
    }
}
