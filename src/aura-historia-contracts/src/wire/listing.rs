//! Canonical listing enum codes at the shared public boundary.
use product_listing_core::{
    listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
    listing_orderability::ListingOrderability,
};
use schemars::{Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
use strum::IntoEnumIterator;

pub mod availability_option {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &Option<ListingAvailability>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&value.map(ListingAvailability::as_str), serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ListingAvailability>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                ListingAvailability::from_code(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .transpose()
    }
}
pub mod lifecycle {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        value: &ListingLifecycle,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }
}
pub fn availability_option_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"anyOf":[{"type":"string","enum":ListingAvailability::iter().map(ListingAvailability::as_str).collect::<Vec<_>>()},{"type":"null"}]})
}
pub fn lifecycle_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type":"string","enum":ListingLifecycle::iter().map(ListingLifecycle::as_str).collect::<Vec<_>>()})
}
fn optional_codes<'de, T: Eq + std::hash::Hash, D: Deserializer<'de>>(
    d: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<HashSet<T>>, D::Error> {
    Option::<Vec<String>>::deserialize(d)?
        .map(|values| {
            values
                .into_iter()
                .map(|value| {
                    parse(&value).ok_or_else(|| {
                        serde::de::Error::custom(format!("unsupported code `{value}`"))
                    })
                })
                .collect()
        })
        .transpose()
}
pub fn availability_set<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<HashSet<ListingAvailability>>, D::Error> {
    optional_codes(d, ListingAvailability::from_code)
}
pub fn orderability_set<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<HashSet<ListingOrderability>>, D::Error> {
    optional_codes(d, ListingOrderability::from_code)
}
pub fn availability_set_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"anyOf":[{"type":"array","items":{"type":"string","enum":ListingAvailability::iter().map(ListingAvailability::as_str).collect::<Vec<_>>()}},{"type":"null"}]})
}
pub fn orderability_set_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"anyOf":[{"type":"array","items":{"type":"string","enum":ListingOrderability::iter().map(ListingOrderability::as_str).collect::<Vec<_>>()}},{"type":"null"}]})
}
