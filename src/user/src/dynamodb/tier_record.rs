use crate::core::tier::UserTier;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserTierRecord {
    Free,
    Pro,
    Ultimate,
}

impl From<UserTierRecord> for UserTier {
    fn from(value: UserTierRecord) -> UserTier {
        match value {
            UserTierRecord::Free => UserTier::Free,
            UserTierRecord::Pro => UserTier::Pro,
            UserTierRecord::Ultimate => UserTier::Ultimate,
        }
    }
}

impl From<UserTier> for UserTierRecord {
    fn from(value: UserTier) -> UserTierRecord {
        match value {
            UserTier::Free => UserTierRecord::Free,
            UserTier::Pro => UserTierRecord::Pro,
            UserTier::Ultimate => UserTierRecord::Ultimate,
        }
    }
}
