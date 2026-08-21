use crate::product_event::{
    domain::ProductDomainEventPayload, enrichment::ProductEnrichmentEventPayload,
    lifecycle::ProductLifecycleEventPayload, policy::ProductPolicyEventPayload,
};
use crate::product_id::ProductId;
use domain_primitives::event::Event;

pub mod domain;
pub mod enrichment;
pub mod lifecycle;
pub mod policy;

pub type ProductDomainEvent = Event<ProductId, ProductDomainEventPayload>;
pub type ProductEnrichmentEvent = Event<ProductId, ProductEnrichmentEventPayload>;
pub type ProductLifecycleEvent = Event<ProductId, ProductLifecycleEventPayload>;
pub type ProductPolicyEvent = Event<ProductId, ProductPolicyEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductEventPayload {
    ProductDomainEvent(ProductDomainEventPayload),
    ProductEnrichmentEvent(ProductEnrichmentEventPayload),
    ProductLifecycleEvent(ProductLifecycleEventPayload),
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

impl From<ProductLifecycleEventPayload> for ProductEventPayload {
    fn from(payload: ProductLifecycleEventPayload) -> Self {
        ProductEventPayload::ProductLifecycleEvent(payload)
    }
}

impl From<ProductPolicyEventPayload> for ProductEventPayload {
    fn from(payload: ProductPolicyEventPayload) -> Self {
        ProductEventPayload::ProductPolicyEvent(payload)
    }
}

impl ProductEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductEventPayload::ProductDomainEvent(payload) => payload.event_type(),
            ProductEventPayload::ProductEnrichmentEvent(payload) => payload.event_type(),
            ProductEventPayload::ProductLifecycleEvent(payload) => payload.event_type(),
            ProductEventPayload::ProductPolicyEvent(payload) => payload.event_type(),
        }
    }

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

    pub fn as_lifecycle_event(&self) -> Option<&ProductLifecycleEventPayload> {
        if let ProductEventPayload::ProductLifecycleEvent(payload) = self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_lifecycle::ProductLifecycle;
    use crate::product_state::ProductState;

    fn domain_payload() -> ProductDomainEventPayload {
        ProductDomainEventPayload::StateChanged(domain::ProductStateChangeDomainEventPayload {
            old_state: ProductState::Listed,
            new_state: ProductState::Available,
        })
    }

    fn lifecycle_payload() -> ProductLifecycleEventPayload {
        ProductLifecycleEventPayload::Deleted(lifecycle::ProductDeletedLifecycleEventPayload {
            old_lifecycle: ProductLifecycle::Active,
            new_lifecycle: ProductLifecycle::Deleted,
        })
    }

    #[test]
    fn should_wrap_and_downcast_event_payloads() {
        let domain = ProductEventPayload::from(domain_payload());
        let lifecycle = ProductEventPayload::from(lifecycle_payload());
        assert!(domain.as_domain_event().is_some());
        assert!(domain.as_enrichment_event().is_none());
        assert!(lifecycle.as_lifecycle_event().is_some());
    }

    #[test]
    fn should_delegate_event_type() {
        let payload = ProductEventPayload::from(domain_payload());

        assert_eq!("DOMAIN_STATE_CHANGED", payload.event_type());
    }
}
