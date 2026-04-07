use crate::core::tier::UserTier;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserTierData {
    Free,
    Pro,
    Ultimate,
}

impl From<UserTierData> for UserTier {
    fn from(value: UserTierData) -> UserTier {
        match value {
            UserTierData::Free => UserTier::Free,
            UserTierData::Pro => UserTier::Pro,
            UserTierData::Ultimate => UserTier::Ultimate,
        }
    }
}

impl From<UserTier> for UserTierData {
    fn from(value: UserTier) -> UserTierData {
        match value {
            UserTier::Free => UserTierData::Free,
            UserTier::Pro => UserTierData::Pro,
            UserTier::Ultimate => UserTierData::Ultimate,
        }
    }
}
