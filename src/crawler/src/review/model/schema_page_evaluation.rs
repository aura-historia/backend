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

#[cfg(test)]
mod tests {
    use super::*;

    /// A matrix cached before the cross-page checks has no `cross_page_findings`; it must
    /// fail to load so the console refreshes it from the live pages instead of showing
    /// it as passing them.
    #[test]
    fn should_not_load_a_cached_page_evaluation_without_cross_page_findings() {
        let mut page = serde_json::json!({
            "page_reference": {"kind": "input", "page_index": 0},
            "url": "https://shop.example/chair",
            "role": "SEED",
            "apply_ok": true,
            "extracted": null,
            "error": null,
            "plausibility_findings": [],
            "cross_page_findings": [],
            "fields": []
        });
        assert!(serde_json::from_value::<SchemaPageEvaluation>(page.clone()).is_ok());

        if let Some(fields) = page.as_object_mut() {
            fields.remove("cross_page_findings");
        }

        assert!(serde_json::from_value::<SchemaPageEvaluation>(page).is_err());
    }
}
