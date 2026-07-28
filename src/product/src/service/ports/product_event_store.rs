#![allow(dead_code)]

use crate::core::product_aggregate::ProductDomainEvent;
use common::event_id::EventId;
use common::product_id::ProductId;

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
pub trait ProductEventStore {
    async fn append(&mut self, event: &ProductDomainEvent) -> Result<(), ProductEventStoreError>;

    async fn find_current_event_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<EventId>, ProductEventStoreError>;
}
