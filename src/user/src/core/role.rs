#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserRole {
    #[default]
    User,
    Admin,
}
