use common::currency::domain::Currency;
use common::price::domain::{MonetaryAmount, Price};
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use product::dynamodb::product_state_record::ProductStateRecord;
use serde::Deserialize;

use crate::expectation_types::{
    NormalizedExpectation, NormalizedExpectationJson, ScraperParsingPipelineFixtureJson,
};

pub struct ScraperParsingPipelineFixture {
    pub raw_state: String,
    pub state_record: String,
    pub raw: crate::expectation_types::RawExpectation,
    pub normalized: NormalizedExpectation,
    pub html_path: String,
}

impl ScraperParsingPipelineFixture {
    pub fn load_html_source(&self) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&self.html_path);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed reading fixture html '{}': {e}", path.display()))
    }

    pub fn state_record_parsed(&self) -> ProductStateRecord {
        match self.state_record.as_str() {
            "AVAILABLE" => ProductStateRecord::Available,
            "LISTED" => ProductStateRecord::Listed,
            "RESERVED" => ProductStateRecord::Reserved,
            "SOLD" => ProductStateRecord::Sold,
            "REMOVED" => ProductStateRecord::Removed,
            "UNKNOWN" => ProductStateRecord::Unknown,
            other => panic!("unsupported state_record '{other}' in fixture json"),
        }
    }
}

pub trait ScraperParsingPipelineCase {
    fn schema(&self) -> ProductCssSelectorSchema;
    fn fixtures(&self) -> Vec<ScraperParsingPipelineFixture>;
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FixtureFile {
    Single(ScraperParsingPipelineFixtureJson),
    Many(Vec<ScraperParsingPipelineFixtureJson>),
}

pub fn load_fixture_json_all(relative_path: &str) -> Vec<ScraperParsingPipelineFixture> {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let raw = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed reading fixture json '{}': {e}", full.display()));
    let parsed: FixtureFile = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("failed parsing fixture json '{}': {e}", full.display()));

    let items = match parsed {
        FixtureFile::Single(one) => vec![one],
        FixtureFile::Many(many) => many,
    };

    assert!(
        !items.is_empty(),
        "fixture json '{}' contains an empty array",
        full.display()
    );

    items
        .into_iter()
        .map(|parsed| ScraperParsingPipelineFixture {
            raw_state: parsed.raw_state,
            state_record: parsed.state_record,
            raw: parsed.raw,
            normalized: normalized_from_json(parsed.normalized),
            html_path: parsed.html,
        })
        .collect()
}

fn normalized_from_json(data: NormalizedExpectationJson) -> NormalizedExpectation {
    NormalizedExpectation {
        shops_product_id: data.shops_product_id,
        title: data.title,
        description: data.description,
        price: price_from_parts(data.price_minor, data.price_currency.as_deref()),
        price_estimate_min: price_from_parts(
            data.price_estimate_min_minor,
            data.price_estimate_min_currency.as_deref(),
        ),
        price_estimate_max: price_from_parts(
            data.price_estimate_max_minor,
            data.price_estimate_max_currency.as_deref(),
        ),
        state: parse_state(&data.state),
        url: data.url,
        images: data.images,
        auction_start: parse_optional_rfc3339(data.auction_start.as_deref()),
        auction_end: parse_optional_rfc3339(data.auction_end.as_deref()),
    }
}

fn price_from_parts(minor: Option<u64>, currency: Option<&str>) -> Option<Price> {
    match (minor, currency) {
        (None, None) => None,
        (Some(amount), Some(curr)) => Some(Price::new(
            MonetaryAmount::from(amount),
            parse_currency(curr),
        )),
        _ => panic!(
            "invalid price representation in fixture json: both minor and currency are required together"
        ),
    }
}

fn parse_currency(code: &str) -> Currency {
    match code {
        "EUR" => Currency::Eur,
        "USD" => Currency::Usd,
        "GBP" => Currency::Gbp,
        "AUD" => Currency::Aud,
        "CAD" => Currency::Cad,
        "NZD" => Currency::Nzd,
        "CNY" => Currency::Cny,
        "BRL" => Currency::Brl,
        "PLN" => Currency::Pln,
        "TRY" => Currency::Try,
        "JPY" => Currency::Jpy,
        "CZK" => Currency::Czk,
        "RUB" => Currency::Rub,
        "AED" => Currency::Aed,
        "SAR" => Currency::Sar,
        "HKD" => Currency::Hkd,
        "SGD" => Currency::Sgd,
        "CHF" => Currency::Chf,
        other => panic!("unsupported currency '{other}' in fixture json"),
    }
}

fn parse_state(state: &str) -> common::product_state::domain::ProductState {
    use common::product_state::domain::ProductState;
    match state {
        "LISTED" => ProductState::Listed,
        "AVAILABLE" => ProductState::Available,
        "RESERVED" => ProductState::Reserved,
        "SOLD" => ProductState::Sold,
        "REMOVED" => ProductState::Removed,
        "UNKNOWN" => ProductState::Unknown,
        other => panic!("unsupported normalized state '{other}' in fixture json"),
    }
}

fn parse_optional_rfc3339(value: Option<&str>) -> Option<time::OffsetDateTime> {
    value.map(|v| {
        time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|e| panic!("invalid RFC3339 datetime '{v}' in fixture json: {e}"))
    })
}
