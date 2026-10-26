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
