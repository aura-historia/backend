#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchFilterState {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl SearchFilterState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_inactive(self) -> bool {
        !self.is_active()
    }
}
