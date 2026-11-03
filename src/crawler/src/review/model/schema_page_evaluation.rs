use super::SelectorFieldEvaluation;
use crate::CrawlerReviewPageId;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use crate::scraper::plausibility::{
    CrossPageFindings, IdentityAgreementFindings, PlausibilityField, PlausibilityFindings,
};
use crate::scraper::raw_input::ForeignPriceCurrency;
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
    /// The `state` label the schema's label mapping does not know, shown next to the
    /// field in the matrix. It never rejects the extraction or blocks approval: the worker
    /// reports a state it cannot resolve, and the reviewer adds the label to the mapping.
    /// A cached matrix without it predates the finding and reads as having none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmapped_state_label: Option<String>,
    /// The price of this extraction that names another currency than the ListingSource's
    /// fallback currency, for the reviewer to see before approving. It is information, not a
    /// check: the capture keeps the value as extracted, so it neither fails the page nor blocks
    /// approval. Set when the matrix is evaluated for a review's live pages. A matrix without it,
    /// such as a cached one or one of generated-schema inputs, reports no foreign currency.
    #[serde(default)]
    pub foreign_price_currency: Option<ForeignPriceCurrencyFinding>,
    pub fields: Vec<SelectorFieldEvaluation>,
}

/// A captured price in another currency than the ListingSource's fallback currency, as the
/// ProductListing normalization worker would read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignPriceCurrencyFinding {
    pub field: PlausibilityField,
    /// ISO 4217 code of the currency the price names.
    pub currency: String,
    /// ISO 4217 code of the ListingSource's fallback currency.
    pub fallback_currency: String,
}

impl From<ForeignPriceCurrency> for ForeignPriceCurrencyFinding {
    fn from(foreign: ForeignPriceCurrency) -> Self {
        Self {
            field: foreign.field,
            currency: foreign.currency.as_str().to_owned(),
            fallback_currency: foreign.fallback_currency.as_str().to_owned(),
        }
    }
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
