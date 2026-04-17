//! General scraper parsing pipeline integration tests.
//!
//! This binary covers **all** registered shops in one parameterised test.
//! Each shop contributes one `ScraperParsingPipelineCase` implementation;
//! adding a new shop requires only:
//!   1. Drop the HTML fixture in  `tests/fixtures/<shop>/`.
//!   2. Add a JSON expectation file in `tests/fixtures/<shop>/` using either
//!      a single object or an array of objects:
//!      [
//!        {
//!          "html": "relative/path/to/html.html",
//!          "raw_state": "...",
//!          "state_record": "...",
//!          "raw": { /* raw expected fields */ },
//!          "normalized": { /* normalized expected fields */ }
//!        }
//!      ]
//!   3. Add a `mod <shop>_case` file under `tests/scraper_parsing_pipeline/`.
//!   4. Implement `ScraperParsingPipelineCase` in that module.
//!
//! All shop cases are instantiated in one vector and tested via loops.

#[path = "scraper_parsing_pipeline/assertions.rs"]
mod assertions;
#[path = "scraper_parsing_pipeline/expectation_types.rs"]
mod expectation_types;
#[path = "scraper_parsing_pipeline/rule_builders.rs"]
mod rule_builders;
#[path = "scraper_parsing_pipeline/scraper_parsing_pipeline_case.rs"]
mod scraper_parsing_pipeline_case;
#[path = "scraper_parsing_pipeline/weitze_case.rs"]
mod weitze_case;

use assertions::{assert_extraction, assert_normalized};
use scraper_parsing_pipeline_case::ScraperParsingPipelineCase;

// ---------------------------------------------------------------------------
// Raw extraction — synchronous
// ---------------------------------------------------------------------------

fn all_shop_cases() -> Vec<Box<dyn ScraperParsingPipelineCase>> {
    vec![Box::new(weitze_case::WeitzeCase)]
}

#[test]
fn should_extract_product_for_all_shops() {
    for shop_case in all_shop_cases() {
        let schema = shop_case.schema();
        for fixture in shop_case.fixtures() {
            let html = fixture.load_html_source();
            assert_extraction(&schema, &html, &fixture.raw);
        }
    }
}

// ---------------------------------------------------------------------------
// Full pipeline — extraction + normalization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn should_normalize_product_for_all_shops() {
    for shop_case in all_shop_cases() {
        let schema = shop_case.schema();
        for fixture in shop_case.fixtures() {
            let html = fixture.load_html_source();
            assert_normalized(
                &schema,
                &html,
                fixture.raw_state.as_str(),
                fixture.state_record_parsed(),
                fixture.normalized.url.as_str(),
                &fixture.normalized,
            )
            .await;
        }
    }
}
