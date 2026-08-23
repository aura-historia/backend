#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnerShopApplicationState {
    Submitted,
    InReview,
    Rejected,
    Approved,
    Withdrawn,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_distinguish_states() {
        assert_ne!(
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::Approved
        );
    }
}
