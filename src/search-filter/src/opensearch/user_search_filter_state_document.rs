use crate::core::user_search_filter_state::UserSearchFilterState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum UserSearchFilterStateDocument {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl UserSearchFilterStateDocument {
    pub fn is_active(&self) -> bool {
        matches!(self, UserSearchFilterStateDocument::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<UserSearchFilterState> for UserSearchFilterStateDocument {
    fn from(state: UserSearchFilterState) -> Self {
        match state {
            UserSearchFilterState::Active => UserSearchFilterStateDocument::Active,
            UserSearchFilterState::InactiveByUser => UserSearchFilterStateDocument::InactiveByUser,
            UserSearchFilterState::InactiveByRestrictedPlan => {
                UserSearchFilterStateDocument::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<UserSearchFilterStateDocument> for UserSearchFilterState {
    fn from(state: UserSearchFilterStateDocument) -> Self {
        match state {
            UserSearchFilterStateDocument::Active => UserSearchFilterState::Active,
            UserSearchFilterStateDocument::InactiveByUser => UserSearchFilterState::InactiveByUser,
            UserSearchFilterStateDocument::InactiveByRestrictedPlan => {
                UserSearchFilterState::InactiveByRestrictedPlan
            }
        }
    }
}
