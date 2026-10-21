use super::*;
use user_core::role::UserRole;

pub fn serialize<S>(value: &UserRole, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serialize_code(value, serializer, UserRole::as_str)
}

pub mod patch {
    use super::*;

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PatchValue<UserRole>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_patch_code(deserializer, UserRole::from_code)
    }
}

pub fn schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    use strum::IntoEnumIterator;
    schemars::json_schema!({"type":"string", "enum": UserRole::iter().map(UserRole::as_str).collect::<Vec<_>>()})
}
