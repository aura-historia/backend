use super::*;
use user_core::tier::UserTier;

pub fn serialize<S>(value: &UserTier, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serialize_code(value, serializer, UserTier::as_str)
}

pub mod patch {
    use super::*;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserTier>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_patch_code(deserializer, UserTier::from_code)
    }
}

pub fn schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    use strum::IntoEnumIterator;
    schemars::json_schema!({"type":"string", "enum": UserTier::iter().map(UserTier::as_str).collect::<Vec<_>>()})
}
