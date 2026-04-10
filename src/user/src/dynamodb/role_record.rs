use crate::core::role::UserRole;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserRoleRecord {
    User,
    Admin,
}

impl UserRoleRecord {
    pub fn default_user() -> Self {
        UserRoleRecord::User
    }
}

impl From<UserRoleRecord> for UserRole {
    fn from(value: UserRoleRecord) -> UserRole {
        match value {
            UserRoleRecord::User => UserRole::User,
            UserRoleRecord::Admin => UserRole::Admin,
        }
    }
}

impl From<UserRole> for UserRoleRecord {
    fn from(value: UserRole) -> UserRoleRecord {
        match value {
            UserRole::User => UserRoleRecord::User,
            UserRole::Admin => UserRoleRecord::Admin,
        }
    }
}
