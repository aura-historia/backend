use crate::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
use common::{
    has_key::HasKey, product_id::ProductKey, shop_id::ShopId, shops_product_id::ShopsProductId,
};

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

impl HasKey for ProductPolicyEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductPolicyEventPayload::ProhibitedContentDecision(payload) => payload.key(),
        }
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProhibitedContentProductPolicyEventPayload {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub decision: ProhibitedContent,
    pub reason: ProhibitedContentReason,
}

impl HasKey for ProhibitedContentProductPolicyEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}
