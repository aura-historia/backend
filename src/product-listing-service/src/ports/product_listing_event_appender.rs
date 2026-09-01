#![allow(dead_code)]

use application::error::BoxError;
use domain_primitives::{event::Event, event_id::EventId};
use product_listing_core::{
    product_listing_event::ProductListingEventPayload, product_listing_id::ProductListingId,
};
use time::OffsetDateTime;

pub type ProductListingEvent = Event<ProductListingId, ProductListingEventPayload>;

pub fn stamp_product_listing_event(
    product_listing_id: ProductListingId,
    occurred_at: OffsetDateTime,
    payload: ProductListingEventPayload,
) -> ProductListingEvent {
    Event {
        aggregate_id: product_listing_id,
        event_id: EventId::new(),
        timestamp: occurred_at,
        payload,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEventAppendError {
    #[error("product listing event already exists")]
    ProductListingEventAlreadyExists,
    #[error("product listing event payload serialization failed")]
    PayloadSerializationFailed {
        #[source]
        source: BoxError,
    },
    #[error("product listing event append failed")]
    ProductListingEventAppendFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ProductListingEventAppender: Send {
    async fn append(
        &mut self,
        event: &ProductListingEvent,
    ) -> Result<(), ProductListingEventAppendError>;
}

pub trait ProductListingEventAppenderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEventAppender + 'tx;
}
