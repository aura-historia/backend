use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use scraper::Html;

// ---------------------------------------------------------------------------
// ExtractionCompletenessScore
// ---------------------------------------------------------------------------

/// Counts distinct populated logical product fields in a raw extraction.
///
/// Each populated logical field contributes exactly one point, regardless of
/// how many values it holds:
///
/// * Image candidates count once when the raw extraction is non-empty. Image
///   validation happens later, only for candidates being normalized.
/// * Multiple description fragments count as one populated `description` field.
/// * Every non-empty `raw_attributes` key counts as one distinct attribute.
///
/// `default_currency` is schema context, not extracted page data, and does not
/// increase the score. Empty strings, empty vectors, and empty optional values
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
        raw.shops_product_id.as_str(),
        raw.title.as_str(),
        raw.state.as_str(),
    ];
    let populated_scalar_strings = scalar_strings
        .iter()
        .filter(|s| !s.trim().is_empty())
        .count();

    // Optional `Option<String>` fields: populated when `Some` and trimmed-non-empty.
    let optional_strings: [Option<&str>; 6] = [
        raw.price.as_deref(),
        raw.price_estimate_min.as_deref(),
        raw.price_estimate_max.as_deref(),
        raw.seller_name.as_deref(),
        raw.auction_start.as_deref(),
        raw.auction_end.as_deref(),
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
/// with its raw extraction and completeness score.
pub(crate) struct AppliedSchemaCandidate<'a> {
    pub schema_index: usize,
    pub schema: &'a ProductCssSelectorSchema,
    pub raw: RawExtractedProduct,
    pub score: ExtractionCompletenessScore,
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
            Ok(raw) => {
                let score = score_raw_product(&raw);
                candidates.push(AppliedSchemaCandidate {
                    schema_index,
                    schema,
                    raw,
                    score,
                });
            }
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

/// Returns applicable cached schema indices in production ranking order.
///
/// This is the shared ranking seam for fixture tests. It applies every schema
/// to one parsed document and performs no network or LLM work.
pub fn rank_applicable_schema_indices(
    schemas: &[ProductCssSelectorSchema],
    html: &str,
) -> Vec<usize> {
    let parsed = Html::parse_document(html);
    let mut candidates = collect_applicable_candidates(schemas, &parsed).candidates;
    rank_candidates(&mut candidates);
    candidates
        .into_iter()
        .map(|candidate| candidate.schema_index)
        .collect()
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

/// Orders candidates by:
///
/// 1. higher completeness score;
/// 2. original schema index as deterministic tie-breaker.
///
/// Stored order therefore affects only ties.
pub(crate) fn rank_candidates(candidates: &mut [AppliedSchemaCandidate<'_>]) {
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then(left.schema_index.cmp(&right.schema_index))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn raw_product() -> RawExtractedProduct {
        RawExtractedProduct {
            shops_product_id: String::new(),
            title: String::new(),
            description: vec![],
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: String::new(),
            images: vec![],
            auction_start: None,
            auction_end: None,
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
        raw.shops_product_id = "SKU-1".to_string();
        raw.title = "Chair".to_string();
        raw.state = "In Stock".to_string();
        assert_eq!(score_raw_product(&raw).as_usize(), 3);
    }

    #[test]
    fn should_not_count_blank_or_whitespace_only_strings() {
        let mut raw = raw_product();
        raw.shops_product_id = "   ".to_string();
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
        raw.seller_name = Some("Seller".to_string());
        raw.auction_start = Some("2024-01-01".to_string());
        raw.auction_end = None;
        // populated: price, seller_name, auction_start
        // empty string / None: price_estimate_max, price_estimate_min, auction_end
        assert_eq!(score_raw_product(&raw).as_usize(), 3);
    }

    #[test]
    fn should_ignore_default_currency_when_scoring() {
        // default_currency lives on ProductCssSelectorSchema, not
        // RawExtractedProduct, so it can never affect the score.
        let raw = raw_product();
        let baseline = score_raw_product(&raw).as_usize();

        let mut populated = raw_product();
        populated.title = "Chair".to_string();
        let populated_baseline = score_raw_product(&populated).as_usize();

        assert_eq!(baseline, 0);
        assert_eq!(populated_baseline, 1);
    }

    #[test]
    fn should_rank_richer_candidate_first() {
        let mut poor = raw_product();
        poor.title = "Chair".to_string();
        let mut rich = raw_product();
        rich.title = "Chair".to_string();
        rich.price = Some("100 EUR".to_string());
        rich.state = "Available".to_string();

        let schema = ProductCssSelectorSchema {
            shops_product_id: None,
            title: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("h1"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::Text,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::First,
            },
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("#state"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::Text,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::First,
            },
            images: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("img"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::ImageUrl,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::All,
            },
            auction_start: None,
            auction_end: None,
            default_currency: None,
            raw_attributes: BTreeMap::new(),
        };

        let mut candidates = vec![
            AppliedSchemaCandidate {
                schema_index: 0,
                schema: &schema,
                raw: poor,
                score: score_raw_product(&raw_product()),
            },
            AppliedSchemaCandidate {
                schema_index: 1,
                schema: &schema,
                raw: rich,
                score: score_raw_product(&raw_product()),
            },
        ];
        candidates[0].score = score_raw_product(&candidates[0].raw);
        candidates[1].score = score_raw_product(&candidates[1].raw);

        rank_candidates(&mut candidates);

        assert_eq!(candidates[0].schema_index, 1);
        assert_eq!(candidates[1].schema_index, 0);
    }

    #[test]
    fn should_break_score_ties_by_original_schema_index() {
        let schema = ProductCssSelectorSchema {
            shops_product_id: None,
            title: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("h1"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::Text,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::First,
            },
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("#state"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::Text,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::First,
            },
            images: crate::scraper::css_selector::rule::ExtractionRule {
                selector: crate::scraper::css_selector::rule::CssSelector::from("img"),
                additional_selectors: vec![],
                extract: crate::scraper::css_selector::rule::ExtractionKind::ImageUrl,
                cardinality: crate::scraper::css_selector::rule::ExtractionCardinality::All,
            },
            auction_start: None,
            auction_end: None,
            default_currency: None,
            raw_attributes: BTreeMap::new(),
        };

        let raw_a = {
            let mut r = raw_product();
            r.title = "Chair".to_string();
            r
        };
        let raw_b = {
            let mut r = raw_product();
            r.title = "Chair".to_string();
            r
        };

        let mut candidates = vec![
            AppliedSchemaCandidate {
                schema_index: 2,
                schema: &schema,
                score: score_raw_product(&raw_a),
                raw: raw_a,
            },
            AppliedSchemaCandidate {
                schema_index: 0,
                schema: &schema,
                score: score_raw_product(&raw_b),
                raw: raw_b,
            },
        ];

        rank_candidates(&mut candidates);

        assert_eq!(candidates[0].schema_index, 0);
        assert_eq!(candidates[1].schema_index, 2);
    }
}
