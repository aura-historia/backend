//! A product-schema matrix as the review tools answer it: each schema's result on each page, mapped
//! from the console's matrix.

use super::{note, timestamp, value};
use crate::review::model::{
    RemovedPageTemplateMatch, RemovedPageTemplateOutcome, SchemaPageEvaluation,
    SchemaPageReference, SelectorFieldEvaluation,
};
use crate::review_mcp::wire;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeMap;

/// A product-schema matrix: every schema evaluated on every page.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MatrixView {
    /// What a live matrix evaluated: the candidate version and when the pages were fetched. Absent
    /// for a proposed candidate's evaluation and for a matrix schema generation stored.
    pub live_evaluation: Option<LiveEvaluationView>,
    /// Review pages this matrix did not evaluate, such as pages that joined later; they count as
    /// not covered until the next refresh.
    pub not_evaluated_pages: Vec<String>,
    pub schemas: Vec<SchemaResultView>,
    /// Pages the candidate's removed-page templates match, and what the scraper makes of them.
    pub removed_page_matches: Vec<RemovedPageMatchView>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LiveEvaluationView {
    pub candidate_version: i64,
    pub fetched_at: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SchemaResultView {
    /// The schema's position in the candidate's `schemas`.
    pub schema_index: usize,
    pub pages: Vec<SchemaPageView>,
}

/// A page a matrix evaluated.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageRefView {
    /// The review page, for a page of the review.
    pub review_page_id: Option<String>,
    /// The position among the `urls` of `evaluate_candidate`, or among the pages schema generation
    /// was given, for a page that is no review page.
    pub input_index: Option<usize>,
    pub url: String,
    pub role: String,
}

/// One schema on one page.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SchemaPageView {
    pub page: PageRefView,
    /// Whether the schema extracts a product from the page that passes every check.
    pub applies: bool,
    pub error: Option<String>,
    pub extracted: Option<ExtractedView>,
    /// How the schema's rules matched the page.
    pub fields: Vec<FieldMatchView>,
    pub findings: Vec<FindingView>,
}

/// What a schema extracted from a page; long values are shortened.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExtractedView {
    pub source_listing_id: String,
    pub title: String,
    pub state: String,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    /// The start of the description's parts, joined.
    pub description_excerpt: String,
    pub description_parts: usize,
    pub images: usize,
    pub first_image: Option<String>,
    /// Each raw attribute's values, joined.
    pub raw_attributes: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FieldMatchView {
    /// The schema field, such as `title` or `raw_attributes.rawMaterial`.
    pub field: String,
    /// Elements the primary selector matched (with visible text, for a text rule); absent when
    /// the selector does not parse.
    pub matches: Option<usize>,
    /// The same count for each additional selector.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub additional_matches: Vec<usize>,
    /// Matched elements of a text rule without visible text.
    #[serde(skip_serializing_if = "is_zero")]
    #[schemars(default)]
    pub empty_elements: usize,
    /// Matched `images` elements without an image URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_elements: Option<usize>,
    /// Whether a miss of this rule fails the page.
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingKind {
    /// A check of the page's own extraction.
    Page,
    /// The value repeats one extracted from another product's page.
    CrossPage,
    /// The extracted identity contradicts another schema's or the captured one.
    Identity,
}

/// A plausibility check an extraction failed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindingView {
    pub kind: FindingKind,
    pub field: String,
    /// The check's code, such as `BLANK` or `REPEATED_ACROSS_PAGES`.
    pub check: String,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(default)]
    pub values: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RemovedPageMatchView {
    pub page: PageRefView,
    /// `REMOVED`: captured as removed; `PRODUCT_STILL_EXTRACTS`: a schema still extracts a
    /// product, so the template is too broad for this page.
    pub outcome: String,
    /// Positions of the matching templates in `removed_page_schemas`.
    pub template_indexes: Vec<usize>,
    pub notices: Vec<String>,
}

/// A matrix evaluated live, with what the answer leaves out.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MatrixAnswer {
    pub matrix: MatrixView,
    pub truncations: Vec<String>,
}

impl MatrixView {
    pub(in crate::review_mcp) fn of(review_matrix: wire::ReviewMatrix) -> Self {
        let wire::ReviewMatrix {
            matrix,
            not_evaluated_pages,
        } = review_matrix;
        Self {
            live_evaluation: matrix.live_evaluation.map(|evaluation| LiveEvaluationView {
                candidate_version: evaluation.candidate_version,
                fetched_at: timestamp(evaluation.fetched_at),
            }),
            not_evaluated_pages: not_evaluated_pages
                .into_iter()
                .map(|page_id| page_id.to_string())
                .collect(),
            schemas: matrix
                .candidates
                .into_iter()
                .map(|candidate| SchemaResultView {
                    schema_index: candidate.schema_index,
                    pages: candidate
                        .pages
                        .into_iter()
                        .map(SchemaPageView::of)
                        .collect(),
                })
                .collect(),
            removed_page_matches: matrix
                .removed_page_matches
                .into_iter()
                .map(RemovedPageMatchView::of)
                .collect(),
        }
    }
}

impl PageRefView {
    fn of(reference: SchemaPageReference, url: String, role: String) -> Self {
        let (review_page_id, input_index) = match reference {
            SchemaPageReference::Persisted { review_page_id } => {
                (Some(review_page_id.to_string()), None)
            }
            SchemaPageReference::Input { page_index } => (None, Some(page_index)),
        };
        Self {
            review_page_id,
            input_index,
            url,
            role,
        }
    }
}

impl SchemaPageView {
    fn of(page: SchemaPageEvaluation) -> Self {
        let findings = page_findings(&page);
        Self {
            page: PageRefView::of(page.page_reference, page.url, page.role),
            applies: page.apply_ok,
            error: page.error.as_deref().map(note),
            extracted: page.extracted.map(ExtractedView::of),
            fields: page.fields.into_iter().map(FieldMatchView::of).collect(),
            findings,
        }
    }
}

fn page_findings(page: &SchemaPageEvaluation) -> Vec<FindingView> {
    let per_page = page
        .plausibility_findings
        .as_slice()
        .iter()
        .map(|finding| FindingView {
            kind: FindingKind::Page,
            field: finding.field.as_str().to_owned(),
            check: code_of(&finding.check),
            message: finding.to_string(),
            values: Vec::new(),
        });
    let cross_page = page
        .cross_page_findings
        .as_slice()
        .iter()
        .map(|finding| FindingView {
            kind: FindingKind::CrossPage,
            field: finding.field.as_str().to_owned(),
            check: code_of(&finding.check),
            message: finding.to_string(),
            values: finding.values.iter().map(|text| value(text)).collect(),
        });
    let identity = page
        .identity_agreement_findings
        .as_slice()
        .iter()
        .map(|finding| FindingView {
            kind: FindingKind::Identity,
            field: finding.field.as_str().to_owned(),
            check: code_of(&finding.check),
            message: finding.to_string(),
            values: finding.values.iter().map(|text| value(text)).collect(),
        });
    per_page.chain(cross_page).chain(identity).collect()
}

/// A check's code as the console persists and answers it.
fn code_of(check: &impl Serialize) -> String {
    serde_json::to_value(check)
        .ok()
        .and_then(|code| code.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl ExtractedView {
    fn of(extracted: RawExtractedProduct) -> Self {
        Self {
            source_listing_id: value(&extracted.source_listing_id),
            title: value(&extracted.title),
            state: value(&extracted.state),
            price: extracted.price.as_deref().map(value),
            price_estimate_min: extracted.price_estimate_min.as_deref().map(value),
            price_estimate_max: extracted.price_estimate_max.as_deref().map(value),
            description_excerpt: value(&extracted.description.join(" ")),
            description_parts: extracted.description.len(),
            images: extracted.images.len(),
            first_image: extracted.images.first().map(|image| value(image)),
            raw_attributes: extracted
                .raw_attributes
                .into_iter()
                .map(|(key, values)| (key, value(&values.join(" | "))))
                .collect(),
        }
    }
}

impl FieldMatchView {
    fn of(field: SelectorFieldEvaluation) -> Self {
        Self {
            field: field.field,
            matches: field.selector_match_count,
            additional_matches: field.additional_selector_match_counts,
            empty_elements: field.empty_element_count,
            skipped_elements: field.skipped_element_count,
            required: field.required,
            error: field.error.as_deref().map(note),
        }
    }
}

impl RemovedPageMatchView {
    fn of(matched: RemovedPageTemplateMatch) -> Self {
        Self {
            page: PageRefView::of(matched.page_reference, matched.url, matched.role),
            outcome: match matched.outcome {
                RemovedPageTemplateOutcome::Removed => "REMOVED",
                RemovedPageTemplateOutcome::ProductStillExtracts => "PRODUCT_STILL_EXTRACTS",
            }
            .to_owned(),
            template_indexes: matched
                .templates
                .iter()
                .map(|template| template.template_index)
                .collect(),
            notices: matched
                .templates
                .iter()
                .flat_map(|template| template.notices.iter().map(|notice| value(notice)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrawlerReviewPageId;
    use serde_json::{Value, json};

    fn decoded<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn should_map_each_schema_page_with_its_extraction_field_matches_and_findings() {
        let page_id = CrawlerReviewPageId::new();
        let matrix: wire::ReviewMatrix = decoded(json!({
            "review_id": null,
            "candidates": [{
                "schema_index": 0,
                "pages": [{
                    "page_reference": { "kind": "persisted", "review_page_id": page_id.to_string() },
                    "url": "https://antiques.example/item/1",
                    "role": "PRIMARY",
                    "apply_ok": false,
                    "extracted": {
                        "source_listing_id": "A1",
                        "title": "",
                        "description": ["Oak", "chair"],
                        "price": "£120",
                        "price_estimate_min": null,
                        "price_estimate_max": null,
                        "state": "In stock",
                        "images": ["a.jpg", "b.jpg"],
                        "raw_attributes": { "rawMaterial": ["oak", "brass"] },
                    },
                    "error": "`title` is blank",
                    "plausibility_findings": [{ "field": "title", "check": "BLANK" }],
                    "cross_page_findings": [{ "field": "price", "check": "REPEATED_ACROSS_PAGES", "values": ["£120"] }],
                    "identity_agreement_findings": [],
                    "fields": [
                        { "field": "title", "selector": "h1", "selector_match_count": 1, "additional_selector_match_counts": [], "error": null, "required": true, "empty_element_count": 1 },
                        { "field": "price", "selector": ".p", "selector_match_count": 0, "additional_selector_match_counts": [2], "error": null, "required": false },
                    ],
                }],
            }],
            "removed_page_matches": [{
                "page_reference": { "kind": "input", "page_index": 1 },
                "url": "https://antiques.example/item/sold",
                "role": "EVALUATED_URL",
                "outcome": "PRODUCT_STILL_EXTRACTS",
                "templates": [{ "template_index": 0, "notices": ["Sold"] }],
            }],
            "live_evaluation": { "candidate_version": 3, "configuration_sha256": "x", "fetched_at": "2026-10-27T12:00:00Z" },
            "not_evaluated_pages": [CrawlerReviewPageId::new().to_string()],
        }));

        let view = json!(MatrixView::of(matrix));

        assert_eq!(
            view["live_evaluation"],
            json!({ "candidate_version": 3, "fetched_at": "2026-10-27T12:00:00Z" })
        );
        assert_eq!(
            view["not_evaluated_pages"].as_array().map(Vec::len),
            Some(1)
        );
        let page = &view["schemas"][0]["pages"][0];
        assert_eq!(
            page["page"],
            json!({ "review_page_id": page_id.to_string(), "input_index": null, "url": "https://antiques.example/item/1", "role": "PRIMARY" })
        );
        assert_eq!(page["applies"], false);
        assert_eq!(page["extracted"]["description_excerpt"], "Oak chair");
        assert_eq!(page["extracted"]["images"], 2);
        assert_eq!(
            page["extracted"]["raw_attributes"],
            json!({ "rawMaterial": "oak | brass" })
        );
        assert_eq!(
            page["fields"],
            json!([
                { "field": "title", "matches": 1, "empty_elements": 1, "required": true },
                { "field": "price", "matches": 0, "additional_matches": [2], "required": false },
            ])
        );
        assert_eq!(
            page["findings"],
            json!([
                { "kind": "PAGE", "field": "title", "check": "BLANK", "message": "`title` is blank" },
                { "kind": "CROSS_PAGE", "field": "price", "check": "REPEATED_ACROSS_PAGES", "message": "`price` repeats a value of another product page", "values": ["£120"] },
            ])
        );
        assert_eq!(
            view["removed_page_matches"][0],
            json!({
                "page": { "review_page_id": null, "input_index": 1, "url": "https://antiques.example/item/sold", "role": "EVALUATED_URL" },
                "outcome": "PRODUCT_STILL_EXTRACTS",
                "template_indexes": [0],
                "notices": ["Sold"],
            })
        );
    }
}
