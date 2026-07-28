#![allow(dead_code)]

use crate::core::product_aggregate::ProductDomainEvent;
use common::event_id::EventId;
use common::product_id::ProductId;

#[derive(Debug, thiserror::Error)]
pub enum ProductEventStoreError {
    #[error("product event already exists")]
    EventConflict,
    #[error("temporary product event persistence failure")]
    TemporarilyUnavailable,
    #[error("invalid product event")]
    InvalidEvent,
    #[error("internal product event persistence failure")]
    Internal,
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for ProductEventStoreError {
    fn from(error: sqlx::Error) -> Self {
        match &error {
            sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
                Self::EventConflict
            }
            _ => Self::Internal,
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait ProductEventStore {
    async fn append(&mut self, event: &ProductDomainEvent) -> Result<(), ProductEventStoreError>;

    async fn find_current_event_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<EventId>, ProductEventStoreError>;
}
