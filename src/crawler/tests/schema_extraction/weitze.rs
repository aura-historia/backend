//! Per-shop extraction test for **Weitze** (weitze.net).
//!
//! Fixture : tests/fixtures/weitze/product.html
//! Product : 405500 — Reichswehr / Wehrmacht Paar Schulterknöpfe …
//!
//! Schema (JSON):
//! ```json
//! {
//!   "shops_product_id": { "type": "text",      "selector": ".nr",                                    "cardinality": "first" },
//!   "title":            { "type": "text",      "selector": "h1[itemprop='name']",                    "cardinality": "first" },
//!   "description":      { "type": "text",      "selector": ".beschreibung[itemprop='description']",  "cardinality": "first" },
//!   "price":            { "type": "text",      "selector": "span[itemprop='price']",                 "cardinality": "first",
//!                         "additional_selectors": [".preis .wert", ".preis"] },
//!   "state":            { "name": "class",     "type": "attribute", "selector": ".cart",              "cardinality": "first",
//!                         "additional_selectors": [".wert"] },
//!   "images":           { "name": "content",   "type": "attribute", "selector": "meta[itemprop='image']", "cardinality": "all" },
//!   "default_currency": "EUR"
//! }
//! ```

use super::common::{
    NormalizedExpectation, RawExpectation, assert_extraction, assert_normalized, attr_rule_all,
    attr_rule_first, text_rule, text_rule_with_fallbacks,
};
use common::currency::domain::Currency;
use common::price::domain::{MonetaryAmount, Price};
use common::product_state::domain::ProductState;
use crawler::scraper::css_selector::currency_dto::CurrencyDto;
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use product::dynamodb::product_state_record::ProductStateRecord;

pub const HTML: &str = include_str!("../fixtures/weitze/product.html");

pub const PRODUCT_URL: &str = "https://www.weitze.net/antiquitaeten/auktionen/Reichswehr_Wehrmacht_Paar_Schulterknopfe_fur_einen_Soldaten_der_6_Kompanie/405500/";

pub fn schema() -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        shops_product_id: text_rule(".nr"),
        title: text_rule("h1[itemprop='name']"),
        description: Some(text_rule(".beschreibung[itemprop='description']")),
        price: Some(text_rule_with_fallbacks(
            "span[itemprop='price']",
            &[".preis .wert", ".preis"],
        )),
        price_estimate_min: None,
        price_estimate_max: None,
        state: attr_rule_first(".cart", "class", &[".wert"]),
        images: attr_rule_all("meta[itemprop='image']", "content"),
        auction_start: None,
        auction_end: None,
        default_currency: Some(CurrencyDto::Eur),
    }
}

pub fn expected() -> RawExpectation {
    RawExpectation {
        shops_product_id: "405500",
        title: "Reichswehr / Wehrmacht Paar Schulterknöpfe für einen Soldaten der 6. Kompanie",
        description: vec!["Aluminium 19 mm, getragen, Zustand 2-."],
        price: Some("25,00"),
        price_estimate_min: None,
        price_estimate_max: None,
        state: "cart",
        images: vec!["https://www.weitze.net/onload/shop/gastfotos/00/405500/405500.webp"],
        auction_start: None,
        auction_end: None,
    }
}

pub fn normalized_expected() -> NormalizedExpectation {
    NormalizedExpectation {
        shops_product_id: "405500",
        title: "Reichswehr / Wehrmacht Paar Schulterknöpfe für einen Soldaten der 6. Kompanie",
        description: Some("Aluminium 19 mm, getragen, Zustand 2-."),
        price: Some(Price::new(MonetaryAmount::from(2500u64), Currency::Eur)),
        price_estimate_min: None,
        price_estimate_max: None,
        state: ProductState::Available,
        url: PRODUCT_URL,
        images: vec!["https://www.weitze.net/onload/shop/gastfotos/00/405500/405500.webp"],
        auction_start: None,
        auction_end: None,
    }
}

#[test]
fn should_extract_product_405500() {
    assert_extraction(&schema(), HTML, &expected());
}

#[tokio::test]
async fn should_normalize_product_405500() {
    assert_normalized(
        &schema(),
        HTML,
        "cart",
        ProductStateRecord::Available,
        PRODUCT_URL,
        &normalized_expected(),
    )
    .await;
}
