use crate::core::partner_shop_application_state::PartnerShopApplicationState;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartnerShopApplicationStateData {
    Submitted,
    InReview,
    Rejected,
    Approved,
}

impl From<PartnerShopApplicationState> for PartnerShopApplicationStateData {
    fn from(state: PartnerShopApplicationState) -> Self {
        match state {
            PartnerShopApplicationState::Submitted => PartnerShopApplicationStateData::Submitted,
            PartnerShopApplicationState::InReview => PartnerShopApplicationStateData::InReview,
            PartnerShopApplicationState::Rejected => PartnerShopApplicationStateData::Rejected,
            PartnerShopApplicationState::Approved => PartnerShopApplicationStateData::Approved,
        }
    }
}

impl From<PartnerShopApplicationStateData> for PartnerShopApplicationState {
    fn from(data: PartnerShopApplicationStateData) -> Self {
        match data {
            PartnerShopApplicationStateData::Submitted => PartnerShopApplicationState::Submitted,
            PartnerShopApplicationStateData::InReview => PartnerShopApplicationState::InReview,
            PartnerShopApplicationStateData::Rejected => PartnerShopApplicationState::Rejected,
            PartnerShopApplicationStateData::Approved => PartnerShopApplicationState::Approved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_state_domain_to_data_and_back() {
        let states = [
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Rejected,
            PartnerShopApplicationState::Approved,
        ];
        for state in states {
            let data: PartnerShopApplicationStateData = state.into();
            let converted: PartnerShopApplicationState = data.into();
            assert_eq!(state, converted);
        }
    }
}
