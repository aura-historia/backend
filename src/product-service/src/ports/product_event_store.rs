#![allow(dead_code)]

use common::event_id::EventId;
use product_core::product::ProductDomainEvent;
use product_core::product_id::ProductId;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventStoreError {
    #[error("product event already exists")]
    ProductEventAlreadyExists,
    #[error("product event append failed")]
    ProductEventAppendFailed,
    #[error("current product event lookup failed")]
    CurrentProductEventLookupFailed,
}

#[async_trait::async_trait]
pub trait ProductEventStore: Send {
    async fn append(&mut self, event: &ProductDomainEvent) -> Result<(), ProductEventStoreError>;

    async fn find_current_event_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<EventId>, ProductEventStoreError>;
}

pub trait ProductEventStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductEventStore + 'tx;
}
