#![allow(dead_code)]

use domain_primitives::event_id::EventId;
use product_listing_core::product_listing::ProductListingDomainEvent;
use product_listing_core::product_listing_id::ProductListingId;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingEventStoreError {
    #[error("product event already exists")]
    ProductListingEventAlreadyExists,
    #[error("product event append failed")]
    ProductListingEventAppendFailed,
    #[error("current product event lookup failed")]
    CurrentProductListingEventLookupFailed,
}

#[async_trait::async_trait]
pub trait ProductListingEventStore: Send {
    async fn append(
        &mut self,
        event: &ProductListingDomainEvent,
    ) -> Result<(), ProductListingEventStoreError>;

    async fn find_current_event_id(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<EventId>, ProductListingEventStoreError>;
}

pub trait ProductListingEventStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductListingEventStore + 'tx;
}
