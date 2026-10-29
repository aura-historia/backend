use super::SelectorFieldEvaluation;
use crate::CrawlerReviewPageId;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::plausibility::{
    CrossPageFindings, IdentityAgreementFindings, PlausibilityFindings,
};
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
    /// The cross-page checks the extraction failed against the candidate's extractions of
    /// the other review pages. They derive from the extractions alone, so reading a cached
    /// matrix evaluates them again (`evaluate_cross_page_findings`), and a check added
    /// after the matrix was cached shows on it. A cached matrix without them predates the
    /// cross-page checks and is refreshed from the live pages.
    pub cross_page_findings: CrossPageFindings,
    /// The identities of this page that contradict the one this extraction derives: other
    /// schemas' and the captured one. A cached matrix without them predates the check and
    /// is refreshed from the live pages.
    pub identity_agreement_findings: IdentityAgreementFindings,
    pub fields: Vec<SelectorFieldEvaluation>,
}

impl SchemaPageEvaluation {
    /// Whether a failed per-page, cross-page or identity agreement check keeps the schema out
    /// of approval. A repeated image alone does not
    /// ([`blocks_approval`](crate::scraper::plausibility::PlausibilityFinding::blocks_approval)).
    pub fn blocks_approval(&self) -> bool {
        self.plausibility_findings.blocks_approval()
            || !self.cross_page_findings.is_empty()
            || !self.identity_agreement_findings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix cached before the cross-page or identity agreement checks lacks their
    /// findings; it must fail to load so the console refreshes it from the live pages
    /// instead of showing it as passing them.
    #[test]
    fn should_not_load_a_cached_page_evaluation_without_later_checks() {
        let page = serde_json::json!({
            "page_reference": {"kind": "input", "page_index": 0},
            "url": "https://shop.example/chair",
            "role": "SEED",
            "apply_ok": true,
            "extracted": null,
            "error": null,
            "plausibility_findings": [],
            "cross_page_findings": [],
            "identity_agreement_findings": [],
            "fields": []
        });
        assert!(serde_json::from_value::<SchemaPageEvaluation>(page.clone()).is_ok());

        for later_check in ["cross_page_findings", "identity_agreement_findings"] {
            let mut cached = page.clone();
            if let Some(fields) = cached.as_object_mut() {
                fields.remove(later_check);
            }
            assert!(
                serde_json::from_value::<SchemaPageEvaluation>(cached).is_err(),
                "a cached page without {later_check} must not load"
            );
        }
    }
}
