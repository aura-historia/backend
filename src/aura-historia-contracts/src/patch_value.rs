use application::patch_field::PatchField;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;

/// A patch member's omission, explicit null, or concrete value.
///
/// Use `#[serde(default)]` on the enclosing DTO member so omission becomes
/// `Omitted`. Without that attribute, Serde accepts omission as `Null`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PatchValue<T> {
    #[default]
    Omitted,
    Null,
    Value(T),
}

impl<T> PatchValue<T> {
    pub fn is_present(&self) -> bool {
        !matches!(self, Self::Omitted)
    }

    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> PatchValue<U> {
        match self {
            Self::Omitted => PatchValue::Omitted,
            Self::Null => PatchValue::Null,
            Self::Value(value) => PatchValue::Value(map(value)),
        }
    }
}

impl<'de, T> Deserialize<'de> for PatchValue<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(deserializer).map(|value| match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

pub fn clearable<T>(value: PatchValue<T>) -> PatchField<T> {
    match value {
        PatchValue::Omitted => PatchField::Unchanged,
        PatchValue::Null => PatchField::Clear,
        PatchValue::Value(value) => PatchField::Set(value),
    }
}

impl<T: JsonSchema> JsonSchema for PatchValue<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("PatchValue_{}", T::schema_name()).into()
    }

    fn schema_id() -> Cow<'static, str> {
        format!("{}::PatchValue<{}>", module_path!(), T::schema_id()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        // Omission is an absent object member, never a third tagged JSON value.
        <Option<T>>::json_schema(generator)
    }

    fn _schemars_private_is_option() -> bool {
        // Serde's missing-field deserializer reaches the Option<T> decoder above.
        true
    }
}

pub mod rfc3339_range {
    use super::PatchValue;
    use domain_primitives::query::range_query::RangeQuery;
    use serde::{Deserialize, Deserializer};
    use time::OffsetDateTime;

    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Rfc3339RangeValue(
        #[serde(with = "domain_primitives::query::range_query::range_rfc3339")]
        RangeQuery<OffsetDateTime>,
    );

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<PatchValue<RangeQuery<OffsetDateTime>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Rfc3339RangeValue>::deserialize(deserializer).map(|value| match value {
            Some(value) => PatchValue::Value(value.0),
            None => PatchValue::Null,
        })
    }
}
