use crate::core::user_search_filter_state::UserSearchFilterState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserSearchFilterStateData {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl UserSearchFilterStateData {
    pub fn is_active(&self) -> bool {
        matches!(self, UserSearchFilterStateData::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<UserSearchFilterState> for UserSearchFilterStateData {
    fn from(state: UserSearchFilterState) -> Self {
        match state {
            UserSearchFilterState::Active => UserSearchFilterStateData::Active,
            UserSearchFilterState::InactiveByUser => UserSearchFilterStateData::InactiveByUser,
            UserSearchFilterState::InactiveByRestrictedPlan => {
                UserSearchFilterStateData::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<UserSearchFilterStateData> for UserSearchFilterState {
    fn from(state: UserSearchFilterStateData) -> Self {
        match state {
            UserSearchFilterStateData::Active => UserSearchFilterState::Active,
            UserSearchFilterStateData::InactiveByUser => UserSearchFilterState::InactiveByUser,
            UserSearchFilterStateData::InactiveByRestrictedPlan => {
                UserSearchFilterState::InactiveByRestrictedPlan
            }
        }
    }
}
