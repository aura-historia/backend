//! Codecs for canonical domain values at the public contract boundary.

pub mod currency;
pub mod description;
pub mod language;

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}

pub mod listing;
pub mod schema;

/// An unsigned amount in minor units that a stored price can hold: signed 64-bit.
pub(crate) fn storage_amount<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<u64, D::Error> {
    let value = <u64 as serde::Deserialize>::deserialize(deserializer)?;
    i64::try_from(value)
        .map_err(|_| serde::de::Error::custom("amount exceeds the supported storage range"))?;
    Ok(value)
}

pub mod source_listing_id {
    pub fn serialize<S: serde::Serializer>(
        value: &product_listing_core::source_listing_id::SourceListingId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_ref())
    }
}

pub mod measurement_unit;
pub mod user_role;
pub mod user_tier;

use crate::patch_value::PatchValue;
use serde::{Deserialize, Deserializer, Serializer};

fn serialize_code<T: Copy, S: Serializer>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(code(*value))
}

fn deserialize_patch_code<'de, T, D: Deserializer<'de>>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<PatchValue<T>, D::Error> {
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |value| {
        parse(&value)
            .map(PatchValue::Value)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
    })
}

fn serialize_option_code<T, S>(
    value: &Option<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(code(*value)),
        None => serializer.serialize_none(),
    }
}

fn deserialize_option_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        parse(&value)
            .map(Some)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
    })
}
