#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WatchlistProductState {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl WatchlistProductState {
    pub fn is_active(&self) -> bool {
        matches!(self, WatchlistProductState::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}
