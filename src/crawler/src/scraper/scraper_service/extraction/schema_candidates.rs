use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::scraper::plausibility::PlausibilityFindings;
use crate::scraper::scraper_service::image_validation::filter_image_urls_offline;
use scraper::Html;

// ---------------------------------------------------------------------------
// ExtractionCompletenessScore
// ---------------------------------------------------------------------------

/// Counts distinct populated logical product fields in a raw extraction.
///
/// Each populated logical field contributes exactly one point, regardless of
/// how many values it holds:
///
/// * Image candidates count once when the extraction contains validated image
///   URLs. The production pipeline validates images before scoring.
/// * Multiple description fragments count as one populated `description` field.
/// * Every non-empty `raw_attributes` key counts as one distinct attribute.
///
/// Empty strings, empty vectors, and empty optional values
/// never count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ExtractionCompletenessScore(usize);

impl ExtractionCompletenessScore {
    pub(crate) fn as_usize(self) -> usize {
        self.0
    }
}

/// Returns true when an extracted value carries real page data, i.e. a
/// non-empty trimmed string. `None` is empty.
fn is_populated(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.trim().is_empty())
}

/// Returns true when at least one extracted value in the list is populated.
fn any_populated(values: &[String]) -> bool {
    values.iter().any(|v| !v.trim().is_empty())
}

/// Score a [`RawExtractedProduct`] by counting distinct populated logical
/// fields. Pure function — no LLM calls, no I/O.
///
/// Each field is encoded exactly once across the category arrays below;
/// the final score is a single sum over them. Adding a new scored field is a
/// one-line change in the relevant array.
pub(crate) fn score_raw_product(raw: &RawExtractedProduct) -> ExtractionCompletenessScore {
    // Mandatory `String` fields: populated when the trimmed value is non-empty.
    let scalar_strings: [&str; 3] = [
        raw.source_listing_id.as_str(),
        raw.title.as_str(),
        raw.state.as_str(),
    ];
    let populated_scalar_strings = scalar_strings
        .iter()
        .filter(|s| !s.trim().is_empty())
        .count();

    // Optional `Option<String>` fields: populated when `Some` and trimmed-non-empty.
    let optional_strings: [Option<&str>; 3] = [
        raw.price.as_deref(),
        raw.price_estimate_min.as_deref(),
        raw.price_estimate_max.as_deref(),
    ];
    let populated_optionals = optional_strings
        .iter()
        .copied()
        .filter(|v| is_populated(*v))
        .count();

    // Multi-valued fields: counted as 1 each, regardless of how many values they hold.
    let populated_descriptions = usize::from(any_populated(&raw.description));
    let populated_images = usize::from(!raw.images.is_empty());
    // Raw attributes: one point per populated key.
    let populated_raw_attribute_keys = raw
        .raw_attributes
        .values()
        .filter(|values| any_populated(values))
        .count();

    ExtractionCompletenessScore(
        populated_scalar_strings
            + populated_optionals
            + populated_descriptions
            + populated_images
            + populated_raw_attribute_keys,
    )
}

// ---------------------------------------------------------------------------
// AppliedSchemaCandidate
// ---------------------------------------------------------------------------

/// A cached schema that applied successfully to the current page, together
/// with its raw extraction.
pub(crate) struct AppliedSchemaCandidate<'a> {
    pub schema_index: usize,
    pub schema: &'a ProductCssSelectorSchema,
    pub raw: RawExtractedProduct,
}

/// An applied candidate that passed the rejecting plausibility checks.
pub(crate) struct CheckedSchemaCandidate<'a> {
    pub schema_index: usize,
    pub schema: &'a ProductCssSelectorSchema,
    /// Untouched extraction retained for source capture.
    pub raw: RawExtractedProduct,
    /// Images that passed crawler validation, in canonical group order.
    pub validated_image_urls: Vec<String>,
    /// Findings that lower the rank or are only reported.
    pub findings: PlausibilityFindings,
    pub score: ExtractionCompletenessScore,
}

impl<'a> CheckedSchemaCandidate<'a> {
    /// Scores the candidate with its validated images, so thumbnails and malformed
    /// URLs cannot inflate its completeness.
    pub(crate) fn new(
        applied: AppliedSchemaCandidate<'a>,
        findings: PlausibilityFindings,
        validated_image_urls: Vec<String>,
    ) -> Self {
        let score = score_raw_product(&RawExtractedProduct {
            images: validated_image_urls.clone(),
            ..applied.raw.clone()
        });
        Self {
            schema_index: applied.schema_index,
            schema: applied.schema,
            raw: applied.raw,
            validated_image_urls,
            findings,
            score,
        }
    }
}

// ---------------------------------------------------------------------------
// SchemaCandidateSet
// ---------------------------------------------------------------------------

/// Result of applying every cached schema to one parsed HTML document.
pub(crate) struct SchemaCandidateSet<'a> {
    pub candidates: Vec<AppliedSchemaCandidate<'a>>,
    /// Diagnostics for schemas that failed to apply, in original schema order.
    pub apply_failures: Vec<(usize, ApplySchemaError)>,
}

/// Applies every schema in `schemas` to the already-parsed `html` document.
///
/// Does not stop at the first successful application — every schema is tried.
/// Schemas that fail application are excluded from the candidate set but their
/// diagnostics are preserved for logging.
pub(crate) fn collect_applicable_candidates<'a>(
    schemas: &'a [ProductCssSelectorSchema],
    html: &Html,
) -> SchemaCandidateSet<'a> {
    let mut candidates = Vec::new();
    let mut apply_failures = Vec::new();

    for (schema_index, schema) in schemas.iter().enumerate() {
        match crate::scraper::scraper_service::extraction::engine::apply_schema_to_document(
            schema, html,
        ) {
            Ok(raw) => candidates.push(AppliedSchemaCandidate {
                schema_index,
                schema,
                raw,
            }),
            Err(err) => {
                apply_failures.push((schema_index, err));
            }
        }
    }

    SchemaCandidateSet {
        candidates,
        apply_failures,
    }
}

/// A cached schema that the fixture ranking seam kept, with its per-page findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedSchemaCandidate {
    pub schema_index: usize,
    pub plausibility_findings: PlausibilityFindings,
}

/// Returns the applicable cached schemas in production ranking order.
///
/// This is the shared ranking seam for fixture tests. It applies every schema
/// to one parsed document, drops candidates that fail a rejecting plausibility
/// check, and performs no network or LLM work. A candidate's images count as
/// capture would count them if every target check and probe accepted: an image
/// URL whose own dimensions show a small image does not count, one that gives
/// no size does.
pub fn rank_applicable_schemas(
    schemas: &[ProductCssSelectorSchema],
    html: &str,
) -> Vec<RankedSchemaCandidate> {
    let parsed = Html::parse_document(html);
    let candidates = collect_applicable_candidates(schemas, &parsed).candidates;
    let base_url = url::Url::parse("https://example.com/").expect("static ranking base URL");
    let mut checked_candidates = Vec::new();
    for candidate in candidates {
        let findings = PlausibilityFindings::of(&candidate.raw);
        if findings.rejects_candidate() {
            continue;
        }
        let validated_image_urls = filter_image_urls_offline(&candidate.raw.images, &base_url);
        checked_candidates.push(CheckedSchemaCandidate::new(
            candidate,
            findings,
            validated_image_urls,
        ));
    }
    rank_checked_candidates(&mut checked_candidates);
    checked_candidates
        .into_iter()
        .map(|candidate| RankedSchemaCandidate {
            schema_index: candidate.schema_index,
            plausibility_findings: candidate.findings,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// Orders candidates by:
///
/// 1. fewer rank-lowering plausibility findings (price, estimates, images);
/// 2. higher completeness score;
/// 3. original schema index as deterministic tie-breaker.
///
/// Stored order therefore affects only ties.
pub(crate) fn rank_checked_candidates(candidates: &mut [CheckedSchemaCandidate<'_>]) {
    candidates.sort_by(|left, right| {
        left.findings
            .rank_penalty()
            .cmp(&right.findings.rank_penalty())
            .then(right.score.cmp(&left.score))
            .then(left.schema_index.cmp(&right.schema_index))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn raw_product() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: String::new(),
            title: String::new(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: String::new(),
            images: vec![],
            raw_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn should_score_empty_product_as_zero() {
        assert_eq!(score_raw_product(&raw_product()).as_usize(), 0);
    }

    #[test]
    fn should_count_each_populated_scalar_field_once() {
        let mut raw = raw_product();
        raw.source_listing_id = "SKU-1".to_string();
        raw.title = "Chair".to_string();
        raw.state = "In Stock".to_string();
        assert_eq!(score_raw_product(&raw).as_usize(), 3);
    }

    #[test]
    fn should_not_count_blank_or_whitespace_only_strings() {
        let mut raw = raw_product();
        raw.source_listing_id = "   ".to_string();
        raw.title = "\n\t".to_string();
        assert_eq!(score_raw_product(&raw).as_usize(), 0);
    }

    #[test]
    fn should_count_many_prepared_images_as_one_field() {
        let mut raw = raw_product();
        raw.images = (0..20).map(|i| format!("img-{i}")).collect();
        assert_eq!(score_raw_product(&raw).as_usize(), 1);
    }

    #[test]
    fn should_count_description_fragments_as_one_field() {
        let mut raw = raw_product();
        raw.description = vec![
            "part 1".to_string(),
            "part 2".to_string(),
            "part 3".to_string(),
        ];
        assert_eq!(score_raw_product(&raw).as_usize(), 1);
    }

    #[test]
    fn should_not_count_all_blank_description_fragments() {
        let mut raw = raw_product();
        raw.description = vec![" ".to_string(), "\n".to_string()];
        assert_eq!(score_raw_product(&raw).as_usize(), 0);
    }

    #[test]
    fn should_count_every_populated_raw_attribute_key_once() {
        let mut raw = raw_product();
        raw.raw_attributes
            .insert("rawMaterial".to_string(), vec!["Oak".to_string()]);
        raw.raw_attributes.insert(
            "rawCondition".to_string(),
            vec!["Good".to_string(), "Worn".to_string()],
        );
        raw.raw_attributes
            .insert("rawYear".to_string(), vec![" ".to_string()]);
        // rawYear values are all blank -> key does not count.
        assert_eq!(score_raw_product(&raw).as_usize(), 2);
    }

    #[test]
    fn should_count_optional_fields_only_when_populated() {
        let mut raw = raw_product();
        raw.price = Some("120 EUR".to_string());
        raw.price_estimate_min = None;
        raw.price_estimate_max = Some("".to_string());
        // Populated: price. Empty strings and absent optional values do not count.
        assert_eq!(score_raw_product(&raw).as_usize(), 1);
    }

    #[test]
    fn should_score_validated_images_instead_of_raw_image_candidates() {
        let schema = schema();
        let mut raw = raw_product();
        raw.title = "Chair".to_string();
        raw.images = vec!["/thumb-100x100.jpg".to_string()];

        let without_valid_image = checked(&schema, 0, raw.clone(), Vec::new());
        let with_valid_image = checked(
            &schema,
            0,
            raw,
            vec!["https://example.com/chair.jpg".to_string()],
        );

        assert_eq!(without_valid_image.score.as_usize(), 1);
        assert_eq!(with_valid_image.score.as_usize(), 2);
        assert_eq!(
            with_valid_image.raw.images,
            vec!["/thumb-100x100.jpg".to_string()],
            "the untouched extraction stays source evidence"
        );
    }

    #[test]
    fn should_rank_richer_candidate_first() {
        let schema = schema();
        let mut poor = raw_product();
        poor.title = "Chair".to_string();
        let mut rich = raw_product();
        rich.title = "Chair".to_string();
        rich.price = Some("100 EUR".to_string());
        rich.state = "Available".to_string();

        let mut candidates = vec![
            checked(&schema, 0, poor, Vec::new()),
            checked(&schema, 1, rich, Vec::new()),
        ];

        rank_checked_candidates(&mut candidates);

        assert_eq!(candidates[0].schema_index, 1);
        assert_eq!(candidates[1].schema_index, 0);
    }

    #[test]
    fn should_rank_candidate_with_fewer_price_or_image_findings_first() {
        let schema = schema();
        let mut zero_price = raw_product();
        zero_price.title = "Chair".to_string();
        zero_price.state = "Sold".to_string();
        zero_price.price = Some("£0.00".to_string());
        zero_price.description = vec!["Oak".to_string()];
        let mut without_price = raw_product();
        without_price.title = "Chair".to_string();
        without_price.state = "Sold".to_string();

        let mut candidates = vec![
            checked(&schema, 0, zero_price, Vec::new()),
            checked(&schema, 1, without_price, Vec::new()),
        ];
        assert!(candidates[0].score > candidates[1].score);

        rank_checked_candidates(&mut candidates);

        assert_eq!(
            candidates[0].schema_index, 1,
            "an implausible price ranks below a richer extraction"
        );
    }

    #[test]
    fn should_break_score_ties_by_original_schema_index() {
        let schema = schema();
        let mut raw = raw_product();
        raw.title = "Chair".to_string();

        let mut candidates = vec![
            checked(&schema, 2, raw.clone(), Vec::new()),
            checked(&schema, 0, raw, Vec::new()),
        ];

        rank_checked_candidates(&mut candidates);

        assert_eq!(candidates[0].schema_index, 0);
        assert_eq!(candidates[1].schema_index, 2);
    }

    #[test]
    fn should_count_thumbnail_named_image_whose_size_needs_a_probe() {
        assert_eq!(
            first_ranked_schema_with_first_image(
                "/uploads/thumbnail_IMG_5849_org_l.jpg?fit=768%2C768"
            ),
            0,
            "both candidates count an image, so stored order breaks the tie"
        );
    }

    #[test]
    fn should_not_count_unparseable_image_candidate() {
        assert_eq!(first_ranked_schema_with_first_image("//"), 1);
    }

    #[test]
    fn should_not_count_image_whose_url_dimensions_show_a_small_image() {
        assert_eq!(
            first_ranked_schema_with_first_image("/uploads/chair-100x100.jpg"),
            1,
            "capture rejects the thumbnail without a probe, so it must not outrank a full-size image"
        );
    }

    /// Ranks two schemas that differ only in their image: schema 0 extracts
    /// `first_image`, schema 1 a plain full-size image.
    fn first_ranked_schema_with_first_image(first_image: &str) -> usize {
        let html = format!(
            r#"<html><body><h1>Chair</h1><span id="state">Available</span>
            <img class="first" src="{first_image}"><img class="second" src="/uploads/chair.jpg">
            </body></html>"#
        );
        let schemas = [
            schema_with_images("img.first"),
            schema_with_images("img.second"),
        ];
        rank_applicable_schemas(&schemas, &html)
            .first()
            .map(|candidate| candidate.schema_index)
            .unwrap_or_else(|| panic!("both schemas must apply"))
    }

    fn schema_with_images(selector: &str) -> ProductCssSelectorSchema {
        let mut schema = schema();
        schema.images.selector = crate::scraper::css_selector::rule::CssSelector::from(selector);
        schema
    }

    fn checked<'a>(
        schema: &'a ProductCssSelectorSchema,
        schema_index: usize,
        raw: RawExtractedProduct,
        validated_image_urls: Vec<String>,
    ) -> CheckedSchemaCandidate<'a> {
        let findings = PlausibilityFindings::of(&raw);
        CheckedSchemaCandidate::new(
            AppliedSchemaCandidate {
                schema_index,
                schema,
                raw,
            },
            findings,
            validated_image_urls,
        )
    }

    fn schema() -> ProductCssSelectorSchema {
        let rule = |selector: &str, extract| crate::scraper::css_selector::rule::ExtractionRule {
            selector: crate::scraper::css_selector::rule::CssSelector::from(selector),
            additional_selectors: vec![],
            extract,
            cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::First,
        };
        ProductCssSelectorSchema {
            source_listing_id: None,
            title: rule(
                "h1",
                crate::scraper::css_selector::rule::ExtractionKind::Text,
            ),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: rule(
                "#state",
                crate::scraper::css_selector::rule::ExtractionKind::Text,
            ),
            images: rule(
                "img",
                crate::scraper::css_selector::rule::ExtractionKind::ImageUrl,
            ),
            raw_attributes: BTreeMap::new(),
        }
    }
}
