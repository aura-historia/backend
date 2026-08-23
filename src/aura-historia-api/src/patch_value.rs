use crate::error::{ApiError, BAD_BODY_VALUE};
use application::patch_field::PatchField;
use serde::{Deserialize, Deserializer};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum PatchValue<T> {
    #[default]
    Omitted,
    Null,
    Value(T),
}

impl<T> PatchValue<T> {
    pub(crate) fn is_present(&self) -> bool {
        !matches!(self, Self::Omitted)
    }

    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> PatchValue<U> {
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

pub(crate) fn clearable<T>(value: PatchValue<T>) -> PatchField<T> {
    match value {
        PatchValue::Omitted => PatchField::Unchanged,
        PatchValue::Null => PatchField::Clear,
        PatchValue::Value(value) => PatchField::Set(value),
    }
}

pub(crate) fn non_nullable_patch<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<PatchField<T>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Value(value) => Ok(PatchField::Set(value)),
        PatchValue::Null => Err(null_not_allowed(field)),
    }
}

pub(crate) fn non_nullable_option<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<Option<T>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(None),
        PatchValue::Value(value) => Ok(Some(value)),
        PatchValue::Null => Err(null_not_allowed(field)),
    }
}

fn null_not_allowed(field: &'static str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE)
        .with_detail(format!("Body field '{field}' must not be null."))
}

pub(crate) mod rfc3339 {
    use super::PatchValue;
    use serde::{Deserialize, Deserializer};
    use time::OffsetDateTime;

    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Rfc3339Value(#[serde(with = "time::serde::rfc3339")] OffsetDateTime);

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<PatchValue<OffsetDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Rfc3339Value>::deserialize(deserializer).map(|value| match value {
            Some(value) => PatchValue::Value(value.0),
            None => PatchValue::Null,
        })
    }
}

pub(crate) mod rfc3339_range {
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

    pub(crate) fn deserialize<'de, D>(
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use time::macros::datetime;

    #[derive(Debug, Deserialize)]
    struct StringPatch {
        #[serde(default)]
        value: PatchValue<String>,
    }

    #[test]
    fn should_decode_omitted_patch_member() -> Result<(), serde_json::Error> {
        let decoded: StringPatch = serde_json::from_str("{}")?;
        assert_eq!(PatchValue::Omitted, decoded.value);
        Ok(())
    }

    #[test]
    fn should_decode_null_patch_member() -> Result<(), serde_json::Error> {
        let decoded: StringPatch = serde_json::from_str(r#"{"value":null}"#)?;
        assert_eq!(PatchValue::Null, decoded.value);
        Ok(())
    }

    #[test]
    fn should_decode_concrete_patch_member() -> Result<(), serde_json::Error> {
        let decoded: StringPatch = serde_json::from_str(r#"{"value":"new"}"#)?;
        assert_eq!(PatchValue::Value("new".to_owned()), decoded.value);
        Ok(())
    }

    #[test]
    fn should_map_value_without_changing_omitted_or_null() {
        assert_eq!(
            PatchValue::<usize>::Omitted,
            PatchValue::<String>::Omitted.map(|value| value.len())
        );
        assert_eq!(
            PatchValue::<usize>::Null,
            PatchValue::<String>::Null.map(|value| value.len())
        );
        assert_eq!(
            PatchValue::Value(3),
            PatchValue::Value("new".to_owned()).map(|value| value.len())
        );
    }

    #[derive(Debug, Deserialize)]
    struct TimePatch {
        #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
        value: PatchValue<time::OffsetDateTime>,
    }

    #[test]
    fn should_decode_rfc3339_patch_value_and_null() -> Result<(), serde_json::Error> {
        let value: TimePatch = serde_json::from_str(r#"{"value":"2026-08-23T12:00:00Z"}"#)?;
        assert_eq!(
            PatchValue::Value(datetime!(2026-08-23 12:00 UTC)),
            value.value
        );

        let null: TimePatch = serde_json::from_str(r#"{"value":null}"#)?;
        assert_eq!(PatchValue::Null, null.value);

        let omitted: TimePatch = serde_json::from_str("{}")?;
        assert_eq!(PatchValue::Omitted, omitted.value);
        Ok(())
    }

    #[derive(Debug, Deserialize)]
    struct TimeRangePatch {
        #[serde(
            default,
            deserialize_with = "crate::patch_value::rfc3339_range::deserialize"
        )]
        value: PatchValue<domain_primitives::query::range_query::RangeQuery<time::OffsetDateTime>>,
    }

    #[test]
    fn should_decode_rfc3339_range_patch_value_null_and_omission() -> Result<(), serde_json::Error>
    {
        let value: TimeRangePatch =
            serde_json::from_str(r#"{"value":{"min":"2026-08-23T12:00:00Z"}}"#)?;
        assert!(matches!(value.value, PatchValue::Value(_)));

        let null: TimeRangePatch = serde_json::from_str(r#"{"value":null}"#)?;
        assert_eq!(PatchValue::Null, null.value);

        let omitted: TimeRangePatch = serde_json::from_str("{}")?;
        assert_eq!(PatchValue::Omitted, omitted.value);
        Ok(())
    }
}
