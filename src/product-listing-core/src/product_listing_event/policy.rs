use crate::prohibited_content::{ProhibitedContent, ProhibitedContentReason};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductListingPolicyEventPayload {
    ProhibitedContentDecision(ProhibitedContentProductListingPolicyEventPayload),
}

impl ProductListingPolicyEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductListingPolicyEventPayload::ProhibitedContentDecision(_) => {
                "POLICY_PROHIBITED_CONTENT_DECISION"
            }
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProhibitedContentProductListingPolicyEventPayload {
    pub decision: ProhibitedContent,
    pub reason: ProhibitedContentReason,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_return_policy_event_type() {
        let event = ProductListingPolicyEventPayload::ProhibitedContentDecision(
            ProhibitedContentProductListingPolicyEventPayload {
                decision: ProhibitedContent::NaziGermany,
                reason: ProhibitedContentReason::ProductListingText,
            },
        );

        assert_eq!("POLICY_PROHIBITED_CONTENT_DECISION", event.event_type());
    }
}
