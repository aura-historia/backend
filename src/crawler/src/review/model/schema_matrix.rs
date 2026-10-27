use super::{RemovedPageTemplateMatch, SchemaCandidateEvaluation};
use crate::CrawlerReviewId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMatrix {
    pub review_id: Option<CrawlerReviewId>,
    pub candidates: Vec<SchemaCandidateEvaluation>,
    /// The review pages the candidate's removed-page templates match; empty without templates.
    #[serde(default)]
    pub removed_page_matches: Vec<RemovedPageTemplateMatch>,
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
