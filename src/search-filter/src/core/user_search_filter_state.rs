#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserSearchFilterState {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl UserSearchFilterState {
    pub fn is_active(&self) -> bool {
        matches!(self, UserSearchFilterState::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}
