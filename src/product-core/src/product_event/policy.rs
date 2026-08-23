use crate::prohibited_content::{ProhibitedContent, ProhibitedContentReason};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductPolicyEventPayload {
    ProhibitedContentDecision(ProhibitedContentProductPolicyEventPayload),
}

impl ProductPolicyEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductPolicyEventPayload::ProhibitedContentDecision(_) => {
                "POLICY_PROHIBITED_CONTENT_DECISION"
            }
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProhibitedContentProductPolicyEventPayload {
    pub decision: ProhibitedContent,
    pub reason: ProhibitedContentReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_policy_event_type() {
        let event = ProductPolicyEventPayload::ProhibitedContentDecision(
            ProhibitedContentProductPolicyEventPayload {
                decision: ProhibitedContent::NaziGermany,
                reason: ProhibitedContentReason::ProductText,
            },
        );

        assert_eq!("POLICY_PROHIBITED_CONTENT_DECISION", event.event_type());
    }
}
