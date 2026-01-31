use crate::core::product_event::{
    domain::ProductDomainEventPayload, policy::ProductPolicyEventPayload,
};
use common::{event::Event, product_id::ProductId};

pub mod domain;
pub mod enrichment;
pub mod policy;

pub type ProductDomainEvent = Event<ProductId, ProductDomainEventPayload>;
pub type ProductPolicyEvent = Event<ProductId, ProductPolicyEventPayload>;
