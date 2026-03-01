use crate::css_selector::rule::ExtractionRule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProductCssSelectorSchema {
    #[schemars(description = "ID of the product on the shop's website")]
    pub shops_product_id: ExtractionRule,

    #[schemars(description = "Title of the product")]
    pub title: ExtractionRule,

    #[schemars(description = "Description of the product. May be fragmented.")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<ExtractionRule>,

    #[schemars(description = "Price of the product.")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<ExtractionRule>,

    #[schemars(
        description = "Lower bound for explicitly mentioned esitmate-price of the product."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<ExtractionRule>,

    #[schemars(
        description = "Upper bound for explicitly mentioned esitmate-price of the product."
    )]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<ExtractionRule>,

    #[schemars(
        description = "Availability state of the product. E.g. 'in stock', 'out of stock', 'preorder', 'add to cart', etc."
    )]
    pub state: ExtractionRule,

    #[schemars(description = "Images of the product. May be fragmented.")]
    pub images: ExtractionRule,

    #[schemars(description = "Start-Date/Time of the auction for the product")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction_start: Option<ExtractionRule>,

    #[schemars(description = "End-Date/Time of the auction for the product")]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction_end: Option<ExtractionRule>,
}

#[cfg(test)]
mod tests {
    use crate::css_selector::product_schema::ProductCssSelectorSchema;

    #[test]
    fn should_foo() {
        let schema = schemars::schema_for!(ProductCssSelectorSchema);
        println!("{}", serde_json::to_string_pretty(&schema).unwrap());
    }
}
