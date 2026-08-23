#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum_macros::EnumIter)]
pub enum SearchFilterState {
    #[default]
    Active,
    InactiveByUser,
    InactiveByRestrictedPlan,
}

impl SearchFilterState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::InactiveByUser => "INACTIVE_BY_USER",
            Self::InactiveByRestrictedPlan => "INACTIVE_BY_RESTRICTED_PLAN",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_inactive(self) -> bool {
        !self.is_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_unique_canonical_state_identifiers() {
        let states = SearchFilterState::iter().collect::<Vec<_>>();
        let identifiers = states
            .iter()
            .map(|state| state.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(states.len(), identifiers.len());
        assert_eq!("ACTIVE", SearchFilterState::Active.as_str());
        assert_eq!(
            "INACTIVE_BY_USER",
            SearchFilterState::InactiveByUser.as_str()
        );
        assert_eq!(
            "INACTIVE_BY_RESTRICTED_PLAN",
            SearchFilterState::InactiveByRestrictedPlan.as_str()
        );
    }
}
