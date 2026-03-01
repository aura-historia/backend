use crate::css_selector::rule::ExtractionRule;
use llm::chat::StructuredOutputFormat;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(
    description = "Schema of rules for extracting product information from a shop's website using CSS selectors.
    Each field represents a specific piece of information about the product, and the value is an ExtractionRule that defines how to extract that information from the HTML of the shop's website.
    The rules are intended to extract raw data from the HTML, not normalized data."
)]
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

impl ProductCssSelectorSchema {
    pub fn structured_output_format() -> StructuredOutputFormat {
        let schema = schemars::schema_for!(ProductCssSelectorSchema);
        let schema_json = serde_json::to_value(&schema).expect(
            "shouldn't fail serializing schema-rs for ProductCssSelectorSchema to json-value",
        );
        StructuredOutputFormat {
            name: "ProductCssSelectorSchema".to_string(),
            description: None,
            schema: Some(schema_json),
            strict: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::css_selector::product_schema::ProductCssSelectorSchema;

    #[test]
    fn should_create_schema() {
        let _schema = schemars::schema_for!(ProductCssSelectorSchema);
        // println!("{}", serde_json::to_string_pretty(&_schema).unwrap());
    }

    #[test]
    fn should_create_structured_output_format() {
        let _structured_output_format = ProductCssSelectorSchema::structured_output_format();
    }
}
