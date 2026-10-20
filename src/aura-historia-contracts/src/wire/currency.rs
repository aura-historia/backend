use money::Currency;
use serde::{Deserializer, Serializer};

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
