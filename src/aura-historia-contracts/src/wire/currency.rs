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

/// Canonical optional currency input and output used by existing query contracts.
pub mod option {
    use money::Currency;
    use schemars::{Schema, SchemaGenerator, json_schema};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Currency>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(Currency::as_str).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Currency>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                Currency::from_code(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .transpose()
    }

    pub fn schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({"anyOf": [super::schema(generator), {"type": "null"}]})
    }
}

pub mod patch {
    use super::*;
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<crate::patch_value::PatchValue<Currency>, D::Error> {
        super::super::deserialize_patch_code(deserializer, Currency::from_code)
    }
}
