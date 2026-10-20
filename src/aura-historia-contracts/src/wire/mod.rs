//! Codecs for canonical domain values at the public contract boundary.

pub mod currency;
pub mod language;

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}
