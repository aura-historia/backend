use crate::core::user_search_filter_state::UserSearchFilterState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserSearchFilterStateRecord {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl UserSearchFilterStateRecord {
    pub fn is_active(&self) -> bool {
        matches!(self, UserSearchFilterStateRecord::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<UserSearchFilterState> for UserSearchFilterStateRecord {
    fn from(state: UserSearchFilterState) -> Self {
        match state {
            UserSearchFilterState::Active => UserSearchFilterStateRecord::Active,
            UserSearchFilterState::InactiveByUser => UserSearchFilterStateRecord::InactiveByUser,
            UserSearchFilterState::InactiveByRestrictedPlan => {
                UserSearchFilterStateRecord::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<UserSearchFilterStateRecord> for UserSearchFilterState {
    fn from(state: UserSearchFilterStateRecord) -> Self {
        match state {
            UserSearchFilterStateRecord::Active => UserSearchFilterState::Active,
            UserSearchFilterStateRecord::InactiveByUser => UserSearchFilterState::InactiveByUser,
            UserSearchFilterStateRecord::InactiveByRestrictedPlan => {
                UserSearchFilterState::InactiveByRestrictedPlan
            }
        }
    }
}
