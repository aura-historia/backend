use crate::core::product_event::domain::ProductDomainEventPayload;
use common::{event::Event, product_id::ProductId};

pub mod domain;

pub type ProductDomainEvent = Event<ProductId, ProductDomainEventPayload>;
