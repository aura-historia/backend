use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crawler::scraper::plausibility::PlausibilityFinding;
use serde::Deserialize;

use crate::expectation_types::RawExpectation;

pub struct ScraperParsingPipelineFixture {
    pub schemas: Vec<ProductCssSelectorSchema>,
    pub schema_index: usize,
    pub raw: RawExpectation,
    pub plausibility_findings: Vec<PlausibilityFinding>,
    pub html_path: String,
}

impl ScraperParsingPipelineFixture {
    pub fn load_html_source(&self) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&self.html_path);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed reading fixture html '{}': {e}", path.display()))
    }
}

#[derive(Debug, Deserialize)]
struct FixtureJson {
    html: String,
    schema: Option<ProductCssSelectorSchema>,
    schemas_file: Option<String>,
    #[serde(default)]
    schema_index: usize,
    raw: RawExpectation,
    /// Per-page plausibility findings of the selected schema; none when omitted.
    #[serde(default)]
    plausibility_findings: Vec<PlausibilityFinding>,
}

/// Load all fixture cases from `tests/fixtures/fixtures.json`.
///
/// Adding a new ListingSource or a new case for an existing ListingSource requires only:
///   1. Drop the HTML file in `tests/fixtures/html/<listing_source>[_variant].html`.
///   2. Add the ListingSource cached schemas to `tests/fixtures/schemas/<listing_source>.json`.
///   3. Append an element to `tests/fixtures/fixtures.json` with
///      `schemas_file`, `schema_index`, `html`, `raw` and, when the page fails a
///      check, `plausibility_findings` fields.
///
/// No Rust code changes are needed.
pub fn load_all_fixtures() -> Vec<ScraperParsingPipelineFixture> {
    let full =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fixtures.json");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed reading '{}': {e}", full.display()));
    let items: Vec<FixtureJson> = serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("failed parsing '{}': {e}", full.display()));

    assert!(
        !items.is_empty(),
        "tests/fixtures/fixtures.json must not be empty"
    );

    items.into_iter().map(fixture_from_json).collect()
}

fn fixture_from_json(f: FixtureJson) -> ScraperParsingPipelineFixture {
    let schemas = match (f.schema, f.schemas_file) {
        (Some(schema), None) => vec![schema],
        (None, Some(path)) => {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            let src = std::fs::read_to_string(&full)
                .unwrap_or_else(|e| panic!("failed reading schemas '{}': {e}", full.display()));
            serde_json::from_str(&src)
                .unwrap_or_else(|e| panic!("failed parsing schemas '{}': {e}", full.display()))
        }
        (Some(_), Some(_)) => panic!("fixture must set only schema or schemas_file"),
        (None, None) => panic!("fixture must set schema or schemas_file"),
    };
    assert!(
        f.schema_index < schemas.len(),
        "schema_index {} is out of range for {} schemas",
        f.schema_index,
        schemas.len()
    );

    ScraperParsingPipelineFixture {
        schemas,
        schema_index: f.schema_index,
        raw: f.raw,
        plausibility_findings: f.plausibility_findings,
        html_path: f.html,
    }
}
