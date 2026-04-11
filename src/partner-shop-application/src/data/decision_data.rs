use crate::core::command::Decision;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DecisionData {
    Approve,
    Reject,
}

impl From<DecisionData> for Decision {
    fn from(data: DecisionData) -> Self {
        match data {
            DecisionData::Approve => Decision::Approve,
            DecisionData::Reject => Decision::Reject,
        }
    }
}

impl From<Decision> for DecisionData {
    fn from(decision: Decision) -> Self {
        match decision {
            Decision::Approve => DecisionData::Approve,
            Decision::Reject => DecisionData::Reject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostDecisionData {
    pub decision: DecisionData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PostDecisionData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostDecisionData {
                decision: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_convert_decision_data_to_domain_and_back() {
        let decisions = [DecisionData::Approve, DecisionData::Reject];
        for data in decisions {
            let domain: Decision = data.into();
            let converted: DecisionData = domain.into();
            assert_eq!(data, converted);
        }
    }

    #[test]
    fn should_deserialize_approve() {
        let json = r#"{"decision":"APPROVE"}"#;
        let data: PostDecisionData = serde_json::from_str(json).unwrap();
        assert_eq!(DecisionData::Approve, data.decision);
    }

    #[test]
    fn should_deserialize_reject() {
        let json = r#"{"decision":"REJECT"}"#;
        let data: PostDecisionData = serde_json::from_str(json).unwrap();
        assert_eq!(DecisionData::Reject, data.decision);
    }
}
