use crate::core::command::PartnerShopApplicationDecision;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartnerShopApplicationDecisionData {
    Approve,
    Reject,
}

impl From<PartnerShopApplicationDecisionData> for PartnerShopApplicationDecision {
    fn from(data: PartnerShopApplicationDecisionData) -> Self {
        match data {
            PartnerShopApplicationDecisionData::Approve => PartnerShopApplicationDecision::Approve,
            PartnerShopApplicationDecisionData::Reject => PartnerShopApplicationDecision::Reject,
        }
    }
}

impl From<PartnerShopApplicationDecision> for PartnerShopApplicationDecisionData {
    fn from(decision: PartnerShopApplicationDecision) -> Self {
        match decision {
            PartnerShopApplicationDecision::Approve => PartnerShopApplicationDecisionData::Approve,
            PartnerShopApplicationDecision::Reject => PartnerShopApplicationDecisionData::Reject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostPartnerShopApplicationDecisionData {
    pub decision: PartnerShopApplicationDecisionData,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for PostPartnerShopApplicationDecisionData {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            PostPartnerShopApplicationDecisionData {
                decision: config.fake_with_rng(rng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_convert_decision_data_to_domain_and_back() {
        let decisions = [
            PartnerShopApplicationDecisionData::Approve,
            PartnerShopApplicationDecisionData::Reject,
        ];
        for data in decisions {
            let domain: PartnerShopApplicationDecision = data.into();
            let converted: PartnerShopApplicationDecisionData = domain.into();
            assert_eq!(data, converted);
        }
    }

    #[test]
    fn should_deserialize_approve() {
        let json = r#"{"decision":"APPROVE"}"#;
        let data: PostPartnerShopApplicationDecisionData = serde_json::from_str(json).unwrap();
        assert_eq!(PartnerShopApplicationDecisionData::Approve, data.decision);
    }

    #[test]
    fn should_deserialize_reject() {
        let json = r#"{"decision":"REJECT"}"#;
        let data: PostPartnerShopApplicationDecisionData = serde_json::from_str(json).unwrap();
        assert_eq!(PartnerShopApplicationDecisionData::Reject, data.decision);
    }

    #[test]
    fn should_roundtrip_decision_data_when_using_screaming_snake_case() {
        let json = json!("APPROVE");
        let data: PartnerShopApplicationDecisionData =
            serde_json::from_value(json.clone()).unwrap();

        assert_eq!(PartnerShopApplicationDecisionData::Approve, data);
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }

    #[test]
    fn should_roundtrip_post_partner_shop_application_decision_data_when_using_camel_case_fields() {
        let json = json!({ "decision": "REJECT" });
        let data: PostPartnerShopApplicationDecisionData =
            serde_json::from_value(json.clone()).unwrap();

        assert_eq!(PartnerShopApplicationDecisionData::Reject, data.decision);
        assert_eq!(json, serde_json::to_value(&data).unwrap());
    }
}
