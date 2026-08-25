use crate::product_listing_event::{
    domain::ProductListingDomainEventPayload, enrichment::ProductListingEnrichmentEventPayload,
    lifecycle::ProductListingLifecycleEventPayload, policy::ProductListingPolicyEventPayload,
};
use crate::product_listing_id::ProductListingId;
use domain_primitives::event::Event;

pub mod domain;
pub mod enrichment;
pub mod lifecycle;
pub mod policy;

pub type ProductListingDomainEvent = Event<ProductListingId, ProductListingDomainEventPayload>;
pub type ProductListingEnrichmentEvent =
    Event<ProductListingId, ProductListingEnrichmentEventPayload>;
pub type ProductListingLifecycleEvent =
    Event<ProductListingId, ProductListingLifecycleEventPayload>;
pub type ProductListingPolicyEvent = Event<ProductListingId, ProductListingPolicyEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductListingEventPayload {
    ProductListingDomainEvent(ProductListingDomainEventPayload),
    ProductListingEnrichmentEvent(ProductListingEnrichmentEventPayload),
    ProductListingLifecycleEvent(ProductListingLifecycleEventPayload),
    ProductListingPolicyEvent(ProductListingPolicyEventPayload),
}

pub type ProductListingEvent = Event<ProductListingId, ProductListingEventPayload>;

impl From<ProductListingDomainEventPayload> for ProductListingEventPayload {
    fn from(payload: ProductListingDomainEventPayload) -> Self {
        ProductListingEventPayload::ProductListingDomainEvent(payload)
    }
}

impl From<ProductListingEnrichmentEventPayload> for ProductListingEventPayload {
    fn from(payload: ProductListingEnrichmentEventPayload) -> Self {
        ProductListingEventPayload::ProductListingEnrichmentEvent(payload)
    }
}

impl From<ProductListingLifecycleEventPayload> for ProductListingEventPayload {
    fn from(payload: ProductListingLifecycleEventPayload) -> Self {
        ProductListingEventPayload::ProductListingLifecycleEvent(payload)
    }
}

impl From<ProductListingPolicyEventPayload> for ProductListingEventPayload {
    fn from(payload: ProductListingPolicyEventPayload) -> Self {
        ProductListingEventPayload::ProductListingPolicyEvent(payload)
    }
}

impl ProductListingEventPayload {
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductListingEventPayload::ProductListingDomainEvent(payload) => payload.event_type(),
            ProductListingEventPayload::ProductListingEnrichmentEvent(payload) => {
                payload.event_type()
            }
            ProductListingEventPayload::ProductListingLifecycleEvent(payload) => {
                payload.event_type()
            }
            ProductListingEventPayload::ProductListingPolicyEvent(payload) => payload.event_type(),
        }
    }

    pub fn as_domain_event(&self) -> Option<&ProductListingDomainEventPayload> {
        if let ProductListingEventPayload::ProductListingDomainEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }

    pub fn as_enrichment_event(&self) -> Option<&ProductListingEnrichmentEventPayload> {
        if let ProductListingEventPayload::ProductListingEnrichmentEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }

    pub fn as_lifecycle_event(&self) -> Option<&ProductListingLifecycleEventPayload> {
        if let ProductListingEventPayload::ProductListingLifecycleEvent(payload) = self {
            Some(payload)
        } else {
            None
        }
    }

    pub fn as_policy_event(&self) -> Option<&ProductListingPolicyEventPayload> {
        if let ProductListingEventPayload::ProductListingPolicyEvent(payload) = self {
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

    fn domain_payload() -> ProductListingDomainEventPayload {
        ProductListingDomainEventPayload::StateChanged(
            domain::ProductListingStateChangeDomainEventPayload {
                old_state: ProductState::Listed,
                new_state: ProductState::Available,
            },
        )
    }

    fn lifecycle_payload() -> ProductListingLifecycleEventPayload {
        ProductListingLifecycleEventPayload::Deleted(
            lifecycle::ProductListingDeletedLifecycleEventPayload {
                old_lifecycle: ProductLifecycle::Active,
                new_lifecycle: ProductLifecycle::Deleted,
            },
        )
    }

    #[test]
    fn should_wrap_and_downcast_event_payloads() {
        let domain = ProductListingEventPayload::from(domain_payload());
        let lifecycle = ProductListingEventPayload::from(lifecycle_payload());
        assert!(domain.as_domain_event().is_some());
        assert!(domain.as_enrichment_event().is_none());
        assert!(lifecycle.as_lifecycle_event().is_some());
    }

    #[test]
    fn should_delegate_event_type() {
        let payload = ProductListingEventPayload::from(domain_payload());

        assert_eq!("DOMAIN_STATE_CHANGED", payload.event_type());
    }
}
