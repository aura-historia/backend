use crate::core::watchlist_product_state::WatchlistProductState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WatchlistProductStateData {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl WatchlistProductStateData {
    pub fn is_active(&self) -> bool {
        matches!(self, WatchlistProductStateData::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}

impl From<WatchlistProductState> for WatchlistProductStateData {
    fn from(state: WatchlistProductState) -> Self {
        match state {
            WatchlistProductState::Active => WatchlistProductStateData::Active,
            WatchlistProductState::InactiveByUser => WatchlistProductStateData::InactiveByUser,
            WatchlistProductState::InactiveByRestrictedPlan => {
                WatchlistProductStateData::InactiveByRestrictedPlan
            }
        }
    }
}

impl From<WatchlistProductStateData> for WatchlistProductState {
    fn from(state: WatchlistProductStateData) -> Self {
        match state {
            WatchlistProductStateData::Active => WatchlistProductState::Active,
            WatchlistProductStateData::InactiveByUser => WatchlistProductState::InactiveByUser,
            WatchlistProductStateData::InactiveByRestrictedPlan => {
                WatchlistProductState::InactiveByRestrictedPlan
            }
        }
    }
}
