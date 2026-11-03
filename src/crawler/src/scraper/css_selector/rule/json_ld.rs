//! JSON-LD `offers` as an extraction source: the availability and price that dealer
//! templates publish only in structured data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How deep the search follows nested objects, `@graph` entries and offers, so a hostile
/// document cannot make extraction recurse without bound.
const MAX_DEPTH: usize = 8;

/// The property of a JSON-LD offer a rule reads.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonLdOfferProperty {
    /// `availability`, such as `https://schema.org/InStock`, as written.
    Availability,
    /// `price`, or `lowPrice` of an `AggregateOffer` without one, as written. The currency
    /// stays out: the source's fallback currency applies, as for a price read from text.
    Price,
}

impl JsonLdOfferProperty {
    fn keys(self) -> &'static [&'static str] {
        match self {
            Self::Availability => &["availability"],
            Self::Price => &["price", "lowPrice"],
        }
    }
}

/// The first non-blank value of `property` among the offers of the JSON-LD document
/// `json`, in document order. `None` when the document does not parse or holds no such
/// value.
pub(super) fn offer_value(json: &str, property: JsonLdOfferProperty) -> Option<String> {
    let document: Value = serde_json::from_str(json.trim()).ok()?;
    find_in_document(&document, property, 0)
}

fn find_in_document(value: &Value, property: JsonLdOfferProperty, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    match value {
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_in_document(item, property, depth + 1)),
        Value::Object(node) => node
            .get("offers")
            .and_then(|offers| find_in_offers(offers, property, depth + 1))
            .or_else(|| {
                node.iter()
                    .filter(|(key, _)| key.as_str() != "offers")
                    .find_map(|(_, child)| find_in_document(child, property, depth + 1))
            }),
        _ => None,
    }
}

fn find_in_offers(offers: &Value, property: JsonLdOfferProperty, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    match offers {
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_in_offers(item, property, depth + 1)),
        Value::Object(offer) => property
            .keys()
            .iter()
            .find_map(|key| offer.get(*key).and_then(scalar))
            .or_else(|| {
                offer
                    .get("offers")
                    .and_then(|nested| find_in_offers(nested, property, depth + 1))
            }),
        _ => None,
    }
}

/// A string or number as written; blank strings and every other JSON type are no value.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.trim().to_owned()).filter(|text| !text.is_empty()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const PRODUCT: &str = r#"{"@context":"https://schema.org","@type":"Product","name":"Chair",
        "offers":{"@type":"Offer","price":"1250.00","priceCurrency":"GBP",
        "availability":"https://schema.org/InStock"}}"#;

    #[rstest]
    #[case(
        PRODUCT,
        JsonLdOfferProperty::Availability,
        Some("https://schema.org/InStock")
    )]
    #[case(PRODUCT, JsonLdOfferProperty::Price, Some("1250.00"))]
    #[case(
        r#"[{"@type":"BreadcrumbList"},{"@type":"Product","offers":[{"price":980,"availability":"SoldOut"}]}]"#,
        JsonLdOfferProperty::Price,
        Some("980")
    )]
    #[case(
        r#"{"@graph":[{"@type":"WebSite"},{"@type":"Product","offers":{"@type":"AggregateOffer","lowPrice":"40","offers":[{"availability":"InStock"}]}}]}"#,
        JsonLdOfferProperty::Availability,
        Some("InStock")
    )]
    #[case(
        r#"{"@type":"Product","offers":{"@type":"AggregateOffer","lowPrice":"40"}}"#,
        JsonLdOfferProperty::Price,
        Some("40")
    )]
    #[case(
        r#"{"@type":"Product","offers":{"price":"  "}}"#,
        JsonLdOfferProperty::Price,
        None
    )]
    #[case(
        r#"{"@type":"Product","name":"No offers"}"#,
        JsonLdOfferProperty::Price,
        None
    )]
    #[case("not json", JsonLdOfferProperty::Availability, None)]
    fn should_read_offer_property(
        #[case] json: &str,
        #[case] property: JsonLdOfferProperty,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(offer_value(json, property).as_deref(), expected);
    }

    #[test]
    fn should_stop_at_the_depth_limit() {
        let nested = format!(
            "{}{{\"offers\":{{\"price\":\"1\"}}}}{}",
            "[".repeat(MAX_DEPTH + 2),
            "]".repeat(MAX_DEPTH + 2)
        );
        assert_eq!(offer_value(&nested, JsonLdOfferProperty::Price), None);
    }
}
