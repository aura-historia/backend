#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceState {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl ResourceState {
    pub fn is_active(&self) -> bool {
        matches!(self, ResourceState::Active)
    }

    pub fn is_inactive(&self) -> bool {
        !self.is_active()
    }
}
