#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum PartnershipApplicationState {
    Submitted,
    InReview,
    Approved,
    Rejected,
    Withdrawn,
}

impl PartnershipApplicationState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InReview => "IN_REVIEW",
            Self::Approved => "APPROVED",
            Self::Rejected => "REJECTED",
            Self::Withdrawn => "WITHDRAWN",
        }
    }
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;
        Self::iter().find(|state| state.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;
    #[test]
    fn should_use_unique_exact_state_codes() {
        let states = PartnershipApplicationState::iter().collect::<Vec<_>>();
        assert_eq!(
            states.len(),
            states
                .iter()
                .map(|state| state.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
        assert_eq!(
            Some(PartnershipApplicationState::InReview),
            PartnershipApplicationState::from_code("IN_REVIEW")
        );
        assert_eq!(None, PartnershipApplicationState::from_code("in_review"));
    }
}
