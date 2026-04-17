//! Per-shop extraction test case for Weitze (weitze.net).

use crawler::scraper::css_selector::currency_dto::CurrencyDto;
use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;

use crate::rule_builders::{attr_rule_all, attr_rule_first, text_rule, text_rule_with_fallbacks};
use crate::scraper_parsing_pipeline_case::{
    ScraperParsingPipelineCase, ScraperParsingPipelineFixture, load_fixture_json_all,
};

pub struct WeitzeCase;

impl ScraperParsingPipelineCase for WeitzeCase {
    fn schema(&self) -> ProductCssSelectorSchema {
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

    fn fixtures(&self) -> Vec<ScraperParsingPipelineFixture> {
        load_fixture_json_all("tests/fixtures/weitze/product.expectation.json")
    }
}
