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

pub mod listing;
pub mod schema;

pub mod source_listing_id {
    pub fn serialize<S: serde::Serializer>(
        value: &product_listing_core::source_listing_id::SourceListingId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_ref())
    }
}
