//! General schema-extraction integration tests.
//!
//! This binary covers **all** registered shops in one parameterised test.
//! Each shop contributes a `(schema, html, expected)` triple via `rstest`;
//! adding a new shop requires only:
//!   1. Drop the HTML fixture in  `tests/fixtures/<shop>/`.
//!   2. Add a `mod <shop>` file under `tests/schema_extraction/`.
//!   3. Add one `#[case::<shop>]` line below.
//!
//! The per-shop modules also contain their own `#[test]` (one per product)
//! so failures are attributed to a specific shop when run individually.

#[path = "schema_extraction/common.rs"]
mod common;
#[path = "schema_extraction/weitze.rs"]
mod weitze;

use common::{NormalizedExpectation, RawExpectation, assert_extraction, assert_normalized};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use product::dynamodb::product_state_record::ProductStateRecord;
use rstest::rstest;

// ---------------------------------------------------------------------------
// Raw extraction — synchronous
// ---------------------------------------------------------------------------

#[rstest]
#[case::weitze_405500(weitze::schema(), weitze::HTML, weitze::expected())]
fn should_extract_product_for_all_shops(
    #[case] schema: ProductCssSelectorSchema,
    #[case] html: &str,
    #[case] expected: RawExpectation,
) {
    assert_extraction(&schema, html, &expected);
}

// ---------------------------------------------------------------------------
// Full pipeline — extraction + normalization
// ---------------------------------------------------------------------------

#[rstest]
#[case::weitze_405500(
    weitze::schema(),
    weitze::HTML,
    "cart",
    ProductStateRecord::Available,
    weitze::PRODUCT_URL,
    weitze::normalized_expected()
)]
#[tokio::test]
async fn should_normalize_product_for_all_shops(
    #[case] schema: ProductCssSelectorSchema,
    #[case] html: &str,
    #[case] raw_state: &'static str,
    #[case] state_record: ProductStateRecord,
    #[case] url: &'static str,
    #[case] expected: NormalizedExpectation,
) {
    assert_normalized(&schema, html, raw_state, state_record, url, &expected).await;
}
