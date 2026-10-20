use money::Currency;
use schemars::{Schema, SchemaGenerator, json_schema};
use serde::{Deserializer, Serializer};
use strum::IntoEnumIterator;

pub fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(value.as_str())
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
where
    D: Deserializer<'de>,
{
    super::deserialize_code(deserializer, Currency::from_code)
}

/// The public currency code vocabulary, shared by input and output schemas.
pub fn schema(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "enum": Currency::iter().map(Currency::as_str).collect::<Vec<_>>()
    })
}
