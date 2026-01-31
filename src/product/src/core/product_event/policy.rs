use crate::core::prohibited_content::ProhibitedContent;
use common::{reason::Reason, shop_id::ShopId, shops_product_id::ShopsProductId};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductPolicyEventPayload {
    ProhibitedContentDecision(ProhibitedContentProductPolicyEventPayload),
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct ProhibitedContentProductPolicyEventPayload {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub decision: ProhibitedContent,
    pub reason: Reason,
}
