use crate::core::watchlist_product_state::WatchlistProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatchlistProductStateRecord {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl WatchlistProductStateRecord {
    pub fn is_active(&self) -> bool {
        matches!(self, WatchlistProductStateRecord::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<WatchlistProductState> for WatchlistProductStateRecord {
    fn from(state: WatchlistProductState) -> Self {
        match state {
            WatchlistProductState::Active => WatchlistProductStateRecord::Active,
            WatchlistProductState::InactiveByUser => WatchlistProductStateRecord::InactiveByUser,
            WatchlistProductState::InactiveByRestrictedPlan => {
                WatchlistProductStateRecord::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<WatchlistProductStateRecord> for WatchlistProductState {
    fn from(state: WatchlistProductStateRecord) -> Self {
        match state {
            WatchlistProductStateRecord::Active => WatchlistProductState::Active,
            WatchlistProductStateRecord::InactiveByUser => WatchlistProductState::InactiveByUser,
            WatchlistProductStateRecord::InactiveByRestrictedPlan => {
                WatchlistProductState::InactiveByRestrictedPlan
            }
        }
    }
}
