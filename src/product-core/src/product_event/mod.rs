use crate::product_event::{
    domain::ProductDomainEventPayload, enrichment::ProductEnrichmentEventPayload,
    lifecycle::ProductLifecycleEventPayload, policy::ProductPolicyEventPayload,
};
use common::{
    has_key::HasKey,
    logging::{LogEventType, LogWriteSource},
    product_id::ProductId,
};
use domain_primitives::{event::Event, event_id::EventId};

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

pub struct ProductEventLog {
    pub event_type: Option<LogEventType>,
    pub write_source: Option<LogWriteSource>,
    pub product_id: ProductId,
    pub event_id: EventId,
    pub product_event_type: String,
    pub decision: Option<String>,
    pub reason: Option<String>,
    pub class: Option<String>,
    pub msg: Option<&'static str>,
}

impl ProductEventLog {
    pub fn with_event_type(mut self, event_type: LogEventType) -> Self {
        self.event_type = Some(event_type);
        self
    }

    pub fn with_write_source(mut self, write_source: LogWriteSource) -> Self {
        self.write_source = Some(write_source);
        self
    }

    pub fn with_msg(mut self, msg: &'static str) -> Self {
        self.msg = Some(msg);
        self
    }

    pub fn log(&self) {
        let event = tracing::info_span!(
            "product_event",
            product_id = %self.product_id,
            event_id = %self.event_id,
            product_event_type = %self.product_event_type,
        );

        if let Some(ref v) = self.event_type {
            event.record("eventType", tracing::field::display(v.as_str()));
        }
        if let Some(ref v) = self.write_source {
            event.record("writeSource", tracing::field::display(v.as_str()));
        }
        if let Some(ref v) = self.decision {
            event.record("decision", tracing::field::display(v));
        }
        if let Some(ref v) = self.reason {
            event.record("reason", tracing::field::display(v));
        }
        if let Some(ref v) = self.class {
            event.record("class", tracing::field::display(v));
        }

        let _entered = event.enter();
        tracing::info!(message = self.msg.unwrap_or(""));
    }
}

impl From<&ProductEvent> for ProductEventLog {
    fn from(event: &ProductEvent) -> Self {
        let (decision, reason, class) = match event.payload {
            ProductEventPayload::ProductDomainEvent(_) => (None, None, None),
            ProductEventPayload::ProductEnrichmentEvent(_) => (None, None, None),
            ProductEventPayload::ProductLifecycleEvent(_) => (None, None, None),
            ProductEventPayload::ProductPolicyEvent(
                ProductPolicyEventPayload::ProhibitedContentDecision(ref payload),
            ) => (
                Some(payload.decision.as_str().to_owned()),
                Some(payload.reason.as_str().to_owned()),
                None,
            ),
        };

        ProductEventLog {
            event_type: None,
            write_source: None,
            product_id: event.aggregate_id,
            event_id: event.event_id,
            product_event_type: event.payload.event_type().to_owned(),
            decision,
            reason,
            class,
            msg: None,
        }
    }
}

impl HasKey for ProductEventLog {
    type Key = ProductId;

    fn key(&self) -> Self::Key {
        self.product_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prohibited_content::{ProhibitedContent, ProhibitedContentReason};
    use common::product_lifecycle::domain::ProductLifecycle;
    use common::product_state::domain::ProductState;
    use time::OffsetDateTime;

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

    fn policy_payload() -> ProductPolicyEventPayload {
        ProductPolicyEventPayload::ProhibitedContentDecision(
            policy::ProhibitedContentProductPolicyEventPayload {
                decision: ProhibitedContent::NaziGermany,
                reason: ProhibitedContentReason::ProductText,
            },
        )
    }

    #[test]
    fn should_wrap_and_downcast_event_payloads() {
        let domain = ProductEventPayload::from(domain_payload());
        let lifecycle = ProductEventPayload::from(lifecycle_payload());
        let policy = ProductEventPayload::from(policy_payload());

        assert!(domain.as_domain_event().is_some());
        assert!(domain.as_enrichment_event().is_none());
        assert!(lifecycle.as_lifecycle_event().is_some());
        assert!(policy.as_policy_event().is_some());
    }

    #[test]
    fn should_delegate_event_type() {
        let payload = ProductEventPayload::from(domain_payload());

        assert_eq!("DOMAIN_STATE_CHANGED", payload.event_type());
    }

    #[test]
    fn should_create_log_from_policy_event_with_envelope_product_id() {
        let event = ProductEvent {
            aggregate_id: ProductId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::from(policy_payload()),
        };

        let log = ProductEventLog::from(&event)
            .with_event_type(LogEventType::PolicyDecision)
            .with_write_source(LogWriteSource::ProductCommandService)
            .with_msg("done");

        assert_eq!(event.aggregate_id, log.product_id);
        assert_eq!(event.event_id, log.event_id);
        assert_eq!("POLICY_PROHIBITED_CONTENT_DECISION", log.product_event_type);
        assert_eq!(Some("NAZI_GERMANY".to_owned()), log.decision);
        assert_eq!(Some("PRODUCT_TEXT".to_owned()), log.reason);
        assert_eq!(event.aggregate_id, log.key());
        assert_eq!(Some("done"), log.msg);
    }
}
