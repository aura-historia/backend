use crate::css_selector::rule::ExtractionRule;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductCssSelectorSchema {
    pub shops_product_id: ExtractionRule,
    pub title: ExtractionRule,
    pub description: ExtractionRule,
    pub price: ExtractionRule,
    pub price_estimate_min: ExtractionRule,
    pub price_estimate_max: ExtractionRule,
    pub state: ExtractionRule,
    pub url: ExtractionRule,
    pub images: ExtractionRule,
    pub auction_start: ExtractionRule,
    pub auction_end: ExtractionRule,
}
