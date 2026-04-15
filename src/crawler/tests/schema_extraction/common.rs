//! Shared helpers for schema-extraction integration tests.
//!
//! Provides:
//! - Rule-builder helpers (`text_rule`, `attr_rule_all`, …)
//! - A `RawExpectation` struct describing the expected raw output for one product
//! - An `assert_extraction` function that applies a schema + fixture HTML and
//!   checks every field against a `RawExpectation`
//! - A `NormalizedExpectation` struct describing the expected normalized output
//! - An `assert_normalized` async function that runs the full extraction +
//!   normalization pipeline (using a mock state mapper) and checks every field

use common::currency::domain::Currency;
use common::price::domain::Price;
use common::product_state::domain::ProductState;
use crawler::scraper::css_selector::product_schema::{
    ProductCssSelectorSchema, RawExtractedProduct,
};
use crawler::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule, HtmlAttributeName,
};
use crawler::scraper::normalization::product_normalization_service::{
    ProductNormalizationService, ProductNormalizationServiceImpl,
};
use crawler::scraper::normalization::state::{ProductStateMappingRecord, StateMappingType};
use crawler::scraper::normalization::state_mapping_service::MockProductStateMappingService;
use product::dynamodb::product_state_record::ProductStateRecord;
use scraper::Html;
use time::OffsetDateTime;
use url::Url;

// ---------------------------------------------------------------------------
// Rule builders
// ---------------------------------------------------------------------------

pub fn text_rule(selector: &str) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

pub fn text_rule_with_fallbacks(selector: &str, additional: &[&str]) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: additional.iter().map(|s| CssSelector::from(*s)).collect(),
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    }
}

pub fn attr_rule_first(selector: &str, attr: &str, additional: &[&str]) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: additional.iter().map(|s| CssSelector::from(*s)).collect(),
        extract: ExtractionKind::Attribute {
            name: HtmlAttributeName::from(attr),
        },
        cardinality: ExtractionCardinality::First,
    }
}

pub fn attr_rule_all(selector: &str, attr: &str) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Attribute {
            name: HtmlAttributeName::from(attr),
        },
        cardinality: ExtractionCardinality::All,
    }
}

// ---------------------------------------------------------------------------
// Raw expected output descriptor
// ---------------------------------------------------------------------------

/// Describes the raw (pre-normalization) values expected from one product page.
pub struct RawExpectation {
    pub shops_product_id: &'static str,
    pub title: &'static str,
    /// Every description fragment, in order.
    pub description: Vec<&'static str>,
    pub price: Option<&'static str>,
    pub price_estimate_min: Option<&'static str>,
    pub price_estimate_max: Option<&'static str>,
    /// The raw string extracted from the state element/attribute.
    pub state: &'static str,
    /// All image URLs / paths extracted by the images rule.
    pub images: Vec<&'static str>,
    pub auction_start: Option<&'static str>,
    pub auction_end: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Normalized expected output descriptor
// ---------------------------------------------------------------------------

/// Describes the fully-normalized values expected after the complete
/// extraction + normalization pipeline for one product page.
///
/// Fields use the same domain types as [`NormalizedProduct`] so the test
/// is as close to production as possible.
pub struct NormalizedExpectation {
    pub shops_product_id: &'static str,
    pub title: &'static str,
    pub description: Option<&'static str>,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub state: ProductState,
    /// The URL used as the product URL during normalization.
    pub url: &'static str,
    /// Expected image URLs in order (only the URL string is checked;
    /// `prohibited_content` is always `Unknown` after normalization).
    pub images: Vec<&'static str>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
}

// ---------------------------------------------------------------------------
// Raw-extraction assertion helper
// ---------------------------------------------------------------------------

/// Applies `schema` to `html_src` and asserts every field matches `expected`.
pub fn assert_extraction(
    schema: &ProductCssSelectorSchema,
    html_src: &str,
    expected: &RawExpectation,
) {
    let html = Html::parse_document(html_src);
    let result: RawExtractedProduct = schema
        .apply(&html)
        .unwrap_or_else(|e| panic!("schema apply failed: {e}"));

    assert_eq!(
        result.shops_product_id, expected.shops_product_id,
        "shops_product_id"
    );
    assert_eq!(result.title, expected.title, "title");
    assert_eq!(result.description, expected.description, "description");
    assert_eq!(result.price.as_deref(), expected.price, "price");
    assert_eq!(
        result.price_estimate_min.as_deref(),
        expected.price_estimate_min,
        "price_estimate_min"
    );
    assert_eq!(
        result.price_estimate_max.as_deref(),
        expected.price_estimate_max,
        "price_estimate_max"
    );
    assert_eq!(result.state, expected.state, "state");
    assert_eq!(result.images, expected.images, "images");
    assert_eq!(
        result.auction_start.as_deref(),
        expected.auction_start,
        "auction_start"
    );
    assert_eq!(
        result.auction_end.as_deref(),
        expected.auction_end,
        "auction_end"
    );
}

// ---------------------------------------------------------------------------
// End-to-end normalization assertion helper
// ---------------------------------------------------------------------------

/// Builds a [`ProductNormalizationServiceImpl`] whose state mapping service
/// resolves `raw_state` (the exact trimmed+lowercased raw string from the
/// fixture) to `state_record`, then runs the full extraction + normalization
/// pipeline and asserts every field against `expected`.
///
/// No database, no LLM, no network — only the mock state mapper is used.
pub async fn assert_normalized(
    schema: &ProductCssSelectorSchema,
    html_src: &str,
    raw_state: &'static str,
    state_record: ProductStateRecord,
    url: &'static str,
    expected: &NormalizedExpectation,
) {
    // 1. Extract raw product from fixture HTML.
    let html = Html::parse_document(html_src);
    let raw = schema
        .apply(&html)
        .unwrap_or_else(|e| panic!("schema apply failed: {e}"));

    // 2. Build normalization service with a mock state mapper.
    let mapping_record = ProductStateMappingRecord {
        raw: raw_state.to_string(),
        normalized: state_record,
        mapping_type: StateMappingType::Value,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let mut mock_mapper = MockProductStateMappingService::new();
    mock_mapper.expect_get_state_mapping().returning(move |_| {
        let r = mapping_record.clone();
        Box::pin(async move { Ok(r) })
    });
    let norm_svc = ProductNormalizationServiceImpl::new(Box::new(mock_mapper));

    // 3. Normalize.
    let product_url = Url::parse(url).expect("test URL must be valid");
    let default_currency = schema.default_currency.map(Currency::from);
    let result = norm_svc
        .normalize(raw, product_url, default_currency)
        .await
        .unwrap_or_else(|e| panic!("normalization failed: {e}"));

    // 4. Assert all fields.
    assert_eq!(
        result.shops_product_id.to_string(),
        expected.shops_product_id,
        "shops_product_id"
    );
    assert_eq!(result.title.payload.as_ref(), expected.title, "title");
    assert_eq!(
        result.description.as_ref().map(|d| d.payload.as_ref()),
        expected.description,
        "description"
    );
    assert_eq!(result.price, expected.price, "price");
    assert_eq!(
        result.price_estimate_min, expected.price_estimate_min,
        "price_estimate_min"
    );
    assert_eq!(
        result.price_estimate_max, expected.price_estimate_max,
        "price_estimate_max"
    );
    assert_eq!(result.state, expected.state, "state");
    assert_eq!(result.url.as_str(), expected.url, "url");
    let result_image_urls: Vec<&str> = result.images.iter().map(|i| i.url.as_str()).collect();
    assert_eq!(result_image_urls, expected.images, "images");
    assert_eq!(
        result.auction_start, expected.auction_start,
        "auction_start"
    );
    assert_eq!(result.auction_end, expected.auction_end, "auction_end");
}
