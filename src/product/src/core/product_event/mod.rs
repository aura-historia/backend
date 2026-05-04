use crate::core::product_event::{
    domain::ProductDomainEventPayload, enrichment::ProductEnrichmentEventPayload,
    policy::ProductPolicyEventPayload,
};
use common::{
    event::Event,
    event_id::EventId,
    has_key::HasKey,
    logging::{LogEventType, LogWriteSource},
    product_id::{ProductId, ProductKey},
    shop_id::ShopId,
    shops_product_id::ShopsProductId,
};

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

impl HasKey for ProductEventPayload {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        match self {
            ProductEventPayload::ProductDomainEvent(event_payload) => event_payload.key(),
            ProductEventPayload::ProductEnrichmentEvent(event_payload) => event_payload.key(),
            ProductEventPayload::ProductPolicyEvent(event_payload) => event_payload.key(),
        }
    }
}

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
    pub fn event_type(&self) -> &'static str {
        match self {
            ProductEventPayload::ProductDomainEvent(payload) => payload.event_type(),
            ProductEventPayload::ProductEnrichmentEvent(payload) => payload.event_type(),
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
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub event_id: EventId,
    pub product_event_type: String,
    pub decision: Option<String>,
    pub reason: Option<String>,
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
            shop_id = %self.shop_id,
            shops_product_id = %self.shops_product_id,
            event_id = %self.event_id,
            product_event_type = %self.product_event_type,
        );

        if let Some(ref v) = self.event_type {
            event.record("event_type", tracing::field::display(v.as_str()));
        }
        if let Some(ref v) = self.write_source {
            event.record("write_source", tracing::field::display(v.as_str()));
        }
        if let Some(ref v) = self.decision {
            event.record("decision", tracing::field::display(v));
        }
        if let Some(ref v) = self.reason {
            event.record("reason", tracing::field::display(v));
        }

        let _entered = event.enter();
        if let Some(v) = self.msg {
            tracing::info!(v);
        } else {
            tracing::info!("");
        }
    }
}

impl From<&ProductEvent> for ProductEventLog {
    fn from(event: &ProductEvent) -> Self {
        let key = event.payload.key();
        let (decision, reason) = match event.payload {
            ProductEventPayload::ProductDomainEvent(_) => (None, None),
            ProductEventPayload::ProductEnrichmentEvent(_) => (None, None),
            ProductEventPayload::ProductPolicyEvent(
                ProductPolicyEventPayload::ProhibitedContentDecision(ref payload),
            ) => (
                Some(payload.decision.as_str().to_owned()),
                Some(payload.reason.as_str().to_owned()),
            ),
        };

        ProductEventLog {
            event_type: None,
            write_source: None,
            product_id: event.aggregate_id,
            shop_id: key.shop_id,
            shops_product_id: key.shops_product_id,
            event_id: event.event_id,
            product_event_type: event.payload.event_type().to_owned(),
            decision,
            reason,
            msg: None,
        }
    }
}

impl HasKey for ProductEventLog {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}
