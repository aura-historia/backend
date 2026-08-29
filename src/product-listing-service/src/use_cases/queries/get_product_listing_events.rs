use crate::ports::{
    ProductListingEventReadError, ProductListingEventReader, ProductListingEventReaderFactory,
};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;

use product_listing_core::product_listing::ProductListingEventPayload;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEventLookup {
    ById(ProductListingId),
    ByTitleSlug(ProductListingSlugId),
}
#[derive(Debug, Clone, PartialEq)]
pub struct GetProductListingEventsRequest {
    pub lookup: ProductListingEventLookup,
}
/// Application-owned history entry with the canonical core payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEvent {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
    pub payload: ProductListingEventPayload,
    pub timestamp: OffsetDateTime,
}
impl ProductListingEvent {
    pub fn event_type(&self) -> &'static str {
        self.payload.event_type()
    }
}
#[derive(Debug, thiserror::Error)]
pub enum GetProductListingEventsError {
    #[error("product listing not found")]
    NotFound,
    #[error("product listing event query failed")]
    QueryFailed,
    #[error("product listing event read model is invalid")]
    InvalidReadModel,
    #[error("failed to begin product listing history transaction")]
    BeginTransactionFailed,
    #[error("failed to commit product listing history transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait GetProductListingEventsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingEventsRequest,
    ) -> Result<Vec<ProductListingEvent>, GetProductListingEventsError>;
}
pub struct GetProductListingEventsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}
impl<U, R> GetProductListingEventsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}
#[async_trait::async_trait]
impl<U, R> GetProductListingEventsUseCase for GetProductListingEventsHandler<U, R>
where
    U: UnitOfWork,
    R: ProductListingEventReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "get_product_listing_history", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductListingEventsRequest,
    ) -> Result<Vec<ProductListingEvent>, GetProductListingEventsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductListingEventsError::BeginTransactionFailed)?;
        let events = self
            .reader
            .in_transaction(&mut tx)
            .find_domain_events(&request.lookup)
            .await?
            .ok_or(GetProductListingEventsError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetProductListingEventsError::CommitTransactionFailed)?;
        Ok(events)
    }
}
impl From<ProductListingEventReadError> for GetProductListingEventsError {
    fn from(error: ProductListingEventReadError) -> Self {
        match error {
            ProductListingEventReadError::ProductListingEventQueryFailed => Self::QueryFailed,
            ProductListingEventReadError::ProductListingEventReadModelInvalid => {
                Self::InvalidReadModel
            }
        }
    }
}
