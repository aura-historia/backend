use crate::core::product_event::policy::ProductPolicyEventPayload;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductPolicyEventTypeRecord {
    PolicyProhibitedContentDecision,
}

impl ProductPolicyEventTypeRecord {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProductPolicyEventTypeRecord::PolicyProhibitedContentDecision => {
                "POLICY_PROHIBITED_CONTENT_DECISION"
            }
        }
    }
}

impl From<&ProductPolicyEventPayload> for ProductPolicyEventTypeRecord {
    fn from(domain: &ProductPolicyEventPayload) -> Self {
        match domain {
            ProductPolicyEventPayload::ProhibitedContentDecision(_) => {
                ProductPolicyEventTypeRecord::PolicyProhibitedContentDecision
            }
        }
    }
}
