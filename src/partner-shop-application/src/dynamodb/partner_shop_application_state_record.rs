use crate::core::partner_shop_application_state::PartnerShopApplicationState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartnerShopApplicationStateRecord {
    Submitted,
    InReview,
    Rejected,
    Approved,
}

impl From<PartnerShopApplicationState> for PartnerShopApplicationStateRecord {
    fn from(state: PartnerShopApplicationState) -> Self {
        match state {
            PartnerShopApplicationState::Submitted => PartnerShopApplicationStateRecord::Submitted,
            PartnerShopApplicationState::InReview => PartnerShopApplicationStateRecord::InReview,
            PartnerShopApplicationState::Rejected => PartnerShopApplicationStateRecord::Rejected,
            PartnerShopApplicationState::Approved => PartnerShopApplicationStateRecord::Approved,
        }
    }
}

impl From<PartnerShopApplicationStateRecord> for PartnerShopApplicationState {
    fn from(record: PartnerShopApplicationStateRecord) -> Self {
        match record {
            PartnerShopApplicationStateRecord::Submitted => PartnerShopApplicationState::Submitted,
            PartnerShopApplicationStateRecord::InReview => PartnerShopApplicationState::InReview,
            PartnerShopApplicationStateRecord::Rejected => PartnerShopApplicationState::Rejected,
            PartnerShopApplicationStateRecord::Approved => PartnerShopApplicationState::Approved,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PartnerShopApplicationStateRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let index: u8 = config.fake_with_rng(rng);
            match index % 4 {
                0 => PartnerShopApplicationStateRecord::Submitted,
                1 => PartnerShopApplicationStateRecord::InReview,
                2 => PartnerShopApplicationStateRecord::Rejected,
                _ => PartnerShopApplicationStateRecord::Approved,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::partner_shop_application_state::PartnerShopApplicationState;

    #[test]
    fn should_convert_state_domain_to_record_and_back() {
        let states = [
            PartnerShopApplicationState::Submitted,
            PartnerShopApplicationState::InReview,
            PartnerShopApplicationState::Rejected,
            PartnerShopApplicationState::Approved,
        ];
        for state in states {
            let record: PartnerShopApplicationStateRecord = state.into();
            let converted: PartnerShopApplicationState = record.into();
            assert_eq!(state, converted);
        }
    }
}
