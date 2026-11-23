//! JSON-LD as an extraction source: the offer availability and price, and the product
//! identifiers, that dealer templates publish only in structured data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod schema_org_product;

pub(crate) use schema_org_product::{is_product_iri, top_level_products};

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
    /// `price`, or `lowPrice` of an `AggregateOffer` without one, as written, followed by
    /// the offer's `priceCurrency` when it has one, so the worker reads the currency the
    /// offer states rather than the source's fallback currency. A price of zero digits only
    /// is no value: templates publish it for price-on-request items, never for a sale. A price
    /// with exactly three decimals is written so the worker reads it as schema.org means it,
    /// or is no value when that meaning is in doubt (see [`offer_price`]).
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

/// The property of the top-level JSON-LD `Product` a rule reads.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonLdProductProperty {
    /// `productID`, as written.
    ProductId,
    /// `sku`, as written.
    Sku,
}

impl JsonLdProductProperty {
    fn key(self) -> &'static str {
        match self {
            Self::ProductId => "productID",
            Self::Sku => "sku",
        }
    }
}

/// The properties through which the search reaches the item a page is about: a `@graph`, a
/// page's `mainEntity` and the variants of a `ProductGroup`. Other properties, such as
/// `isRelatedTo` or an `ItemList`'s `itemListElement`, name other products, whose offers
/// would be captured as this one's.
const ITEM_PROPERTIES: [&str; 3] = ["@graph", "mainEntity", "hasVariant"];

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
                ITEM_PROPERTIES
                    .iter()
                    .filter_map(|key| node.get(*key))
                    .find_map(|child| find_in_document(child, property, depth + 1))
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
        Value::Object(offer) => offer_property(offer, property).or_else(|| {
            offer
                .get("offers")
                .and_then(|nested| find_in_offers(nested, property, depth + 1))
        }),
        _ => None,
    }
}

/// `property` of one offer object, without looking into nested offers.
fn offer_property(
    offer: &serde_json::Map<String, Value>,
    property: JsonLdOfferProperty,
) -> Option<String> {
    let value = property
        .keys()
        .iter()
        .find_map(|key| offer.get(*key).filter(|value| scalar(value).is_some()))?;
    match property {
        JsonLdOfferProperty::Availability => scalar(value),
        JsonLdOfferProperty::Price => {
            let price = offer_price(value).filter(|price| !has_only_zero_digits(price))?;
            Some(
                match offer
                    .get("priceCurrency")
                    .and_then(Value::as_str)
                    .map(str::trim)
                {
                    Some(currency) if !currency.is_empty() => format!("{price} {currency}"),
                    _ => price,
                },
            )
        }
    }
}

/// An offer price as the worker reads it. schema.org writes a price as a decimal with a dot
/// (`1250.00`), which the worker's display-price reading agrees with, except that it groups
/// thousands by a dot that exactly three digits follow. Such a price is therefore written as the
/// decimal it is: without its trailing zeros (`1250.000` gives `1250`, the number `12.500` gives
/// `12.5`), or with one more when it has none (`1250.345` gives `1250.3450`, which the worker
/// rejects as excess precision). A JSON number is always such a decimal. A string of one to
/// three whole digits and three decimals (`"12.500"`) is a decimal to schema.org but grouped
/// thousands to many dealers, so it is no value and the rule's next selector decides. Every
/// other price is as written.
fn offer_price(value: &Value) -> Option<String> {
    let written = scalar(value)?;
    let Some((whole, decimals)) = written.split_once('.').filter(|(whole, decimals)| {
        decimals.len() == 3
            && !whole.is_empty()
            && whole
                .bytes()
                .chain(decimals.bytes())
                .all(|byte| byte.is_ascii_digit())
    }) else {
        return Some(written);
    };
    let reads_as_thousands = whole.len() <= 3 && !whole.starts_with('0');
    if reads_as_thousands && !value.is_number() {
        return None;
    }
    let decimals = decimals.trim_end_matches('0');
    Some(match decimals.len() {
        0 => whole.to_owned(),
        3 => format!("{whole}.{decimals}0"),
        _ => format!("{whole}.{decimals}"),
    })
}

/// Whether `value` holds digits and every one of them is a zero, such as `0` or `0.00`.
fn has_only_zero_digits(value: &str) -> bool {
    let mut digits = value.chars().filter(char::is_ascii_digit).peekable();
    digits.peek().is_some() && digits.all(|digit| digit == '0')
}

/// The first non-blank value of `property` among the top-level schema.org `Product` nodes of the
/// JSON-LD document `json`, in document order: the document, the members of an array and of a
/// `@graph`, never a product nested in another node. `None` when the document does not parse or
/// no such product holds the value.
pub(in crate::scraper::css_selector) fn product_value(
    json: &str,
    property: JsonLdProductProperty,
) -> Option<String> {
    let document: Value = serde_json::from_str(json.trim()).ok()?;
    top_level_products(&document)
        .into_iter()
        .find_map(|product| product.get(property.key()).and_then(scalar))
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
    #[case(PRODUCT, JsonLdOfferProperty::Price, Some("1250.00 GBP"))]
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
    #[case::main_entity_and_variants(
        r#"{"@type":"WebPage","mainEntity":{"@type":"ProductGroup","hasVariant":[{"@type":"Product","offers":{"price":"310"}}]}}"#,
        JsonLdOfferProperty::Price,
        Some("310")
    )]
    #[case::not_a_related_product(
        r#"{"@type":"Product","isRelatedTo":{"@type":"Product","offers":{"availability":"InStock"}}}"#,
        JsonLdOfferProperty::Availability,
        None
    )]
    #[case::not_a_listed_product(
        r#"{"@graph":[{"@type":"ItemList","itemListElement":[{"@type":"ListItem","item":{"@type":"Product","offers":{"price":"99"}}}]},{"@type":"Product","offers":{"price":"1850"}}]}"#,
        JsonLdOfferProperty::Price,
        Some("1850")
    )]
    #[case::zero_price_is_price_on_request(
        r#"{"@type":"Product","offers":{"price":"0.00","priceCurrency":"GBP","availability":"InStock"}}"#,
        JsonLdOfferProperty::Price,
        None
    )]
    #[case::zero_number_price(r#"{"offers":{"price":0}}"#, JsonLdOfferProperty::Price, None)]
    #[case::next_offer_after_a_zero_price(
        r#"{"offers":[{"price":"0.00"},{"price":"75.50","priceCurrency":"USD"}]}"#,
        JsonLdOfferProperty::Price,
        Some("75.50 USD")
    )]
    #[case::availability_of_a_zero_price_offer(
        r#"{"offers":{"price":"0.00","availability":"https://schema.org/InStock"}}"#,
        JsonLdOfferProperty::Availability,
        Some("https://schema.org/InStock")
    )]
    #[case::currency_of_the_aggregate_offer(
        r#"{"offers":{"@type":"AggregateOffer","lowPrice":40,"priceCurrency":" EUR "}}"#,
        JsonLdOfferProperty::Price,
        Some("40 EUR")
    )]
    #[case::non_string_currency_is_left_out(
        r#"{"offers":{"price":"12","priceCurrency":{"code":"EUR"}}}"#,
        JsonLdOfferProperty::Price,
        Some("12")
    )]
    fn should_read_offer_property(
        #[case] json: &str,
        #[case] property: JsonLdOfferProperty,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(offer_value(json, property).as_deref(), expected);
    }

    #[rstest]
    // A decimal whose three decimals a display price would read as grouped thousands.
    #[case::trailing_zeros(r#""1250.000""#, Some("1250"))]
    #[case::one_trailing_zero(r#""1250.500""#, Some("1250.5"))]
    #[case::no_whole_units(r#""0.500""#, Some("0.5"))]
    #[case::excess_precision(r#""1250.345""#, Some("1250.3450"))]
    #[case::number(r#"12.345"#, Some("12.3450"))]
    // A string schema.org reads as a decimal and many dealers as grouped thousands.
    #[case::ambiguous_thousands(r#""125.000""#, None)]
    #[case::ambiguous_decimal(r#""12.500""#, None)]
    #[case::ambiguous_small(r#""1.250""#, None)]
    // Every other price as written.
    #[case::two_decimals(r#""1250.00""#, Some("1250.00"))]
    #[case::one_decimal(r#""12.5""#, Some("12.5"))]
    #[case::dealer_grouping(r#""1,250.00""#, Some("1,250.00"))]
    #[case::four_decimals(r#""12.3450""#, Some("12.3450"))]
    fn should_write_a_three_decimal_price_as_the_decimal_it_is(
        #[case] price: &str,
        #[case] expected: Option<&str>,
    ) {
        let json = format!(r#"{{"@type":"Product","offers":{{"price":{price}}}}}"#);
        assert_eq!(
            offer_value(&json, JsonLdOfferProperty::Price).as_deref(),
            expected,
            "{price}"
        );
    }

    #[test]
    fn should_not_read_the_low_price_in_place_of_an_ambiguous_price() {
        assert_eq!(
            offer_value(
                r#"{"offers":{"@type":"AggregateOffer","price":"12.500","lowPrice":"9.50"}}"#,
                JsonLdOfferProperty::Price
            ),
            None
        );
    }

    #[rstest]
    #[case::product_id(
        r#"{"@type":"Product","productID":"9441317814427","sku":"R-1"}"#,
        JsonLdProductProperty::ProductId,
        Some("9441317814427")
    )]
    #[case::sku(
        r#"{"@type":"Product","productID":"9441317814427","sku":"R-1"}"#,
        JsonLdProductProperty::Sku,
        Some("R-1")
    )]
    #[case::number_as_written(
        r#"{"@type":"Product","sku":9441317814427}"#,
        JsonLdProductProperty::Sku,
        Some("9441317814427")
    )]
    #[case::blank_is_no_value(
        r#"{"@type":"Product","sku":"  "}"#,
        JsonLdProductProperty::Sku,
        None
    )]
    #[case::absent(
        r#"{"@type":"Product","name":"Chair"}"#,
        JsonLdProductProperty::ProductId,
        None
    )]
    #[case::unparseable("not json", JsonLdProductProperty::Sku, None)]
    #[case::first_non_blank_in_document_order(
        r#"[{"@type":"Product","sku":" "},{"@type":"Product","sku":"B-2"},{"@type":"Product","sku":"C-3"}]"#,
        JsonLdProductProperty::Sku,
        Some("B-2")
    )]
    #[case::in_a_graph(
        r#"{"@context":"https://schema.org","@graph":[{"@type":"WebSite"},{"@type":"Product","sku":"G-1"}]}"#,
        JsonLdProductProperty::Sku,
        Some("G-1")
    )]
    #[case::not_a_node_of_another_type(
        r#"{"@type":"WebPage","sku":"W-1"}"#,
        JsonLdProductProperty::Sku,
        None
    )]
    #[case::not_a_nested_product(
        r#"{"@type":"ItemList","itemListElement":[{"@type":"Product","sku":"L-1"}]}"#,
        JsonLdProductProperty::Sku,
        None
    )]
    #[case::not_a_product_of_another_vocabulary(
        r#"{"@context":"https://example.org/vocab","@type":"Product","sku":"X-1"}"#,
        JsonLdProductProperty::Sku,
        None
    )]
    #[case::object_is_no_value(
        r#"{"@type":"Product","sku":{"@value":"S-1"}}"#,
        JsonLdProductProperty::Sku,
        None
    )]
    fn should_read_product_property(
        #[case] json: &str,
        #[case] property: JsonLdProductProperty,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(product_value(json, property).as_deref(), expected);
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
