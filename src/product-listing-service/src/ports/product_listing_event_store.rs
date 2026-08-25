#![allow(dead_code)]

use domain_primitives::{event::Event, event_id::EventId};
use product_listing_core::{
    product_listing::ProductListingEventPayload, product_listing_id::ProductListingId,
};
use time::OffsetDateTime;

pub type ProductListingEvent = Event<ProductListingId, ProductListingEventPayload>;

pub fn stamp_product_listing_events(
    product_listing_id: ProductListingId,
    occurred_at: OffsetDateTime,
    payloads: Vec<ProductListingEventPayload>,
) -> Vec<ProductListingEvent> {
    payloads
        .into_iter()
        .map(|payload| Event {
            aggregate_id: product_listing_id,
            event_id: EventId::new(),
            timestamp: occurred_at,
            payload,
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEventStoreError {
    #[error("product listing event already exists")]
    ProductListingEventAlreadyExists,
    #[error("product listing event append failed")]
    ProductListingEventAppendFailed,
    #[error("current product listing event lookup failed")]
    CurrentProductListingEventLookupFailed,
}

#[async_trait::async_trait]
pub trait ProductListingEventStore: Send {
    async fn append(
        &mut self,
        event: &ProductListingEvent,
    ) -> Result<(), ProductListingEventStoreError>;

    async fn find_current_event_id(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<EventId>, ProductListingEventStoreError>;
}

pub trait ProductListingEventStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEventStore + 'tx;
}
