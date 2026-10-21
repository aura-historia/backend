use super::*;
fn serialize_optional_set_code<T, S>(
    values: &Option<HashSet<T>>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + std::hash::Hash,
    S: Serializer,
{
    match values {
        Some(values) => serializer.collect_seq(values.iter().map(|value| code(*value))),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<HashSet<T>>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(None), |values| {
        values
            .into_iter()
            .map(|value| {
                parse(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(Some)
    })
}

fn deserialize_patch_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<PatchValue<HashSet<T>>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: Deserializer<'de>,
{
    Option::<Vec<String>>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |values| {
        values
            .into_iter()
            .map(|value| {
                parse(&value)
                    .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
            })
            .collect::<Result<HashSet<_>, D::Error>>()
            .map(PatchValue::Value)
    })
}

pub(super) mod listing_availability_set_option {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &Option<HashSet<ListingAvailability>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_optional_set_code(values, serializer, ListingAvailability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<HashSet<ListingAvailability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_optional_set_code(deserializer, ListingAvailability::from_code)
    }
}

pub(super) mod listing_orderability_set_option {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &Option<HashSet<ListingOrderability>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_optional_set_code(values, serializer, ListingOrderability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Option<HashSet<ListingOrderability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_optional_set_code(deserializer, ListingOrderability::from_code)
    }
}

pub(super) mod listing_orderability_patch_set {
    use super::*;

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<PatchValue<HashSet<ListingOrderability>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_patch_set_code(deserializer, ListingOrderability::from_code)
    }
}

pub(super) fn language_patch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PatchValue<Language>, D::Error> {
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |code| {
        crate::wire::language::parse(&code)
            .map(PatchValue::Value)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{code}`")))
    })
}
pub(super) fn currency_patch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PatchValue<Currency>, D::Error> {
    Option::<String>::deserialize(deserializer)?.map_or(Ok(PatchValue::Null), |code| {
        Currency::from_code(&code)
            .map(PatchValue::Value)
            .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{code}`")))
    })
}
pub(super) fn availability_patch<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<PatchValue<HashSet<ListingAvailability>>, D::Error> {
    deserialize_patch_set_code(deserializer, ListingAvailability::from_code)
}
pub(super) mod state {
    use super::*;
    pub fn serialize<S: Serializer>(
        value: &SearchFilterState,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(value.as_str())
    }
}
fn nullable(schema: schemars::Schema) -> schemars::Schema {
    schemars::json_schema!({"anyOf":[schema,{"type":"null"}]})
}
pub(super) fn text_queries(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"array","items":{"type":"string","minLength":1}})
}
pub(super) fn nullable_auction_ids(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    nullable(crate::wire::schema::auction_ids(generator))
}
pub(super) fn state_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    use strum::IntoEnumIterator;
    schemars::json_schema!({"type":"string","enum":SearchFilterState::iter().map(SearchFilterState::as_str).collect::<Vec<_>>()})
}
pub(super) fn user_id(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    crate::wire::schema::object_id(user_core::user_id::UserId::PREFIX)
}
pub(super) fn address_id(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    crate::wire::schema::object_id(user_core::user_address::UserAddressId::PREFIX)
}

pub(super) fn atomic_availability_patch(schema: &mut schemars::Schema) {
    schema.insert("anyOf".into(), serde_json::json!([
        {"not":{"anyOf":[{"required":["availability"]},{"required":["orderability"]},{"required":["includeUnspecifiedAvailability"]}]}},
        {"required":["availability","orderability","includeUnspecifiedAvailability"], "properties":{"availability":{"type":"null"},"orderability":{"type":"null"},"includeUnspecifiedAvailability":{"type":"null"}}},
        {"required":["availability","orderability","includeUnspecifiedAvailability"], "properties":{"availability":{"type":"array"},"orderability":{"type":"array"},"includeUnspecifiedAvailability":{"type":"boolean"}}}
    ]));
}
