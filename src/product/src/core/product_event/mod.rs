use crate::core::product_event::{
    domain::ProductDomainEventPayload, enrichment::ProductEnrichmentEventPayload,
    policy::ProductPolicyEventPayload,
};
use common::{event::Event, product_id::ProductId};

pub mod domain;
pub mod enrichment;
pub mod policy;

pub type ProductDomainEvent = Event<ProductId, ProductDomainEventPayload>;
pub type ProductEnrichmentEvent = Event<ProductId, ProductEnrichmentEventPayload>;
pub type ProductPolicyEvent = Event<ProductId, ProductPolicyEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEventPayload {
    ProductDomainEvent(ProductDomainEventPayload),
    ProductEnrichmentEvent(ProductEnrichmentEventPayload),
    ProductPolicyEvent(ProductPolicyEventPayload),
}

pub type ProductEvent = Event<ProductId, ProductEventPayload>;

impl From<ProductDomainEventPayload> for ProductEventPayload {
    fn from(payload: ProductDomainEventPayload) -> Self {
        ProductEventPayload::ProductDomainEvent(payload)
    }
}

impl From<ProductEnrichmentEventPayload> for ProductEventPayload {
    fn from(payload: ProductEnrichmentEventPayload) -> Self {
        ProductEventPayload::ProductEnrichmentEvent(payload)
    }
}

impl From<ProductPolicyEventPayload> for ProductEventPayload {
    fn from(payload: ProductPolicyEventPayload) -> Self {
        ProductEventPayload::ProductPolicyEvent(payload)
    }
}

impl ProductEventPayload {
    pub fn as_domain_event(&self) -> Option<&ProductDomainEventPayload> {
        if let ProductEventPayload::ProductDomainEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }

    pub fn as_enrichment_event(&self) -> Option<&ProductEnrichmentEventPayload> {
        if let ProductEventPayload::ProductEnrichmentEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }

    pub fn as_policy_event(&self) -> Option<&ProductPolicyEventPayload> {
        if let ProductEventPayload::ProductPolicyEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }
}
