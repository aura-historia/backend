use super::SelectorFieldEvaluation;
use crate::CrawlerReviewPageId;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::plausibility::{CrossPageFindings, PlausibilityFindings};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaPageReference {
    Persisted { review_page_id: CrawlerReviewPageId },
    Input { page_index: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaPageEvaluation {
    pub page_reference: SchemaPageReference,
    pub url: String,
    pub role: String,
    pub apply_ok: bool,
    pub extracted: Option<RawExtractedProduct>,
    pub error: Option<String>,
    /// Every per-page plausibility check the extraction failed. A cached matrix
    /// without them predates the checks and is refreshed from the live pages.
    pub plausibility_findings: PlausibilityFindings,
    /// The values the extraction shares with the candidate's extraction of another
    /// review page. A cached matrix without them predates the cross-page checks and is
    /// refreshed from the live pages.
    pub cross_page_findings: CrossPageFindings,
    pub fields: Vec<SelectorFieldEvaluation>,
}

impl SchemaPageEvaluation {
    /// Whether the extraction failed a per-page or a cross-page plausibility check.
    pub fn fails_plausibility_checks(&self) -> bool {
        !self.plausibility_findings.is_empty() || !self.cross_page_findings.is_empty()
    }
}
