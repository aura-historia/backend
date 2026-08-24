#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum PartnerShopApplicationState {
    Submitted,
    InReview,
    Rejected,
    Approved,
    Withdrawn,
}

impl PartnerShopApplicationState {
    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|state| state.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InReview => "IN_REVIEW",
            Self::Rejected => "REJECTED",
            Self::Approved => "APPROVED",
            Self::Withdrawn => "WITHDRAWN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_define_unique_canonical_state_identifiers() {
        let states = PartnerShopApplicationState::iter().collect::<Vec<_>>();
        let identifiers = states
            .iter()
            .map(|state| state.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(states.len(), identifiers.len());
        assert_eq!(
            vec![
                "SUBMITTED",
                "IN_REVIEW",
                "REJECTED",
                "APPROVED",
                "WITHDRAWN"
            ],
            states
                .into_iter()
                .map(PartnerShopApplicationState::as_str)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn should_round_trip_canonical_partner_application_state_identifiers() {
        for state in PartnerShopApplicationState::iter() {
            assert_eq!(
                Some(state),
                PartnerShopApplicationState::from_code(state.as_str())
            );
        }
        assert_eq!(None, PartnerShopApplicationState::from_code("submitted"));
    }
}
