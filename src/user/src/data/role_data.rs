use crate::core::role::UserRole;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserRoleData {
    User,
    Admin,
}

impl From<UserRoleData> for UserRole {
    fn from(value: UserRoleData) -> UserRole {
        match value {
            UserRoleData::User => UserRole::User,
            UserRoleData::Admin => UserRole::Admin,
        }
    }
}

impl From<UserRole> for UserRoleData {
    fn from(value: UserRole) -> UserRoleData {
        match value {
            UserRole::User => UserRoleData::User,
            UserRole::Admin => UserRoleData::Admin,
        }
    }
}
