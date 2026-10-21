use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, RawExtractedProduct,
};
use crawler::scraper::plausibility::PlausibilityFinding;
use crawler::scraper::scraper_service::rank_applicable_schemas;
use scraper::Html;

use crate::expectation_types::RawExpectation;

pub fn assert_extraction(
    schemas: &[ProductCssSelectorSchema],
    schema_index: usize,
    html_src: &str,
    expected: &RawExpectation,
) {
    let html = Html::parse_document(html_src);
    let ranked = rank_applicable_schemas(schemas, html_src);
    assert_eq!(
        ranked.first().map(|candidate| candidate.schema_index),
        Some(schema_index)
    );
    let schema = schemas
        .get(schema_index)
        .unwrap_or_else(|| panic!("schema index {schema_index} out of range"));
    let result: RawExtractedProduct = schema
        .apply(&html)
        .unwrap_or_else(|e| panic!("schema apply failed: {e}"));

    assert_eq!(
        result.source_listing_id, expected.source_listing_id,
        "source_listing_id"
    );
    assert_eq!(result.title, expected.title, "title");
    assert_eq!(result.description, expected.description, "description");
    assert_eq!(result.price.as_deref(), expected.price.as_deref(), "price");
    assert_eq!(
        result.price_estimate_min.as_deref(),
        expected.price_estimate_min.as_deref(),
        "price_estimate_min"
    );
    assert_eq!(
        result.price_estimate_max.as_deref(),
        expected.price_estimate_max.as_deref(),
        "price_estimate_max"
    );
    assert_eq!(result.state, expected.state, "state");
    assert_eq!(result.images, expected.images, "images");
}

pub fn assert_plausibility_findings(
    schemas: &[ProductCssSelectorSchema],
    schema_index: usize,
    html_src: &str,
    expected: &[PlausibilityFinding],
    fixture: &str,
) {
    let selected = rank_applicable_schemas(schemas, html_src)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{fixture}: a cached schema must pass the rejecting checks"));
    assert_eq!(selected.schema_index, schema_index, "{fixture}");
    assert_eq!(
        selected.plausibility_findings.as_slice(),
        expected,
        "{fixture}"
    );
}
