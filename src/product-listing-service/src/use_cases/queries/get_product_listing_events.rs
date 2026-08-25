use crate::ports::{
    ProductListingEventReadError, ProductListingEventReader, ProductListingEventReaderFactory,
};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::product_state::ProductState;
use shop_core::shop_slug_id::ShopSlugId;

use indexmap::IndexSet;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingPricing, ProductSaleValuation,
};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::{description::Description, title::Title};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEventLookup {
    ById(ProductListingId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductListingSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductListingEventsRequest {
    pub lookup: ProductListingEventLookup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingEvent {
    pub product_id: ProductListingId,
    pub event_id: EventId,
    pub event_type: ProductListingEventType,
    pub payload: ProductListingEventPayload,
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingEventType {
    Created,
    StateChanged,
    AddressChanged,
    PriceChanged,
    UrlChanged,
    ImagesChanged,
    AuctionChanged,
    Deleted,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ProductListingEventPayload {
    Created(ProductListingCreatedEventPayload),
    StateChanged(ProductListingStateChangedEventPayload),
    AddressChanged(ProductListingAddressChangedEventPayload),
    PriceChanged(ProductListingPriceChangedEventPayload),
    UrlChanged(ProductListingUrlChangedEventPayload),
    ImagesChanged(ProductListingImagesChangedEventPayload),
    AuctionChanged(ProductListingAuctionChangedEventPayload),
    Deleted(ProductListingDeletedEventPayload),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingCreatedEventPayload {
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub address: ProductListingAddress,
    pub pricing: ProductListingPricing,
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductListingImage>,
    pub auction: ProductListingAuction,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingStateChangedEventPayload {
    pub old_state: ProductState,
    pub new_state: ProductState,
    pub sale_valuation: Option<ProductSaleValuation>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingAddressChangedEventPayload {
    pub address: ProductListingAddress,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPriceChangedEventPayload {
    pub old_pricing: ProductListingPricing,
    pub new_pricing: ProductListingPricing,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingUrlChangedEventPayload {
    pub old_url: Url,
    pub new_url: Url,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingImagesChangedEventPayload {
    pub images: IndexSet<ProductListingImage>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingAuctionChangedEventPayload {
    pub auction: ProductListingAuction,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDeletedEventPayload {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductListingEventsError {
    #[error("product not found")]
    NotFound,
    #[error("product event query failed")]
    ProductListingEventQueryFailed,
    #[error("product event read model is invalid")]
    ProductListingEventReadModelInvalid,
    #[error("failed to begin get product events transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product events transaction")]
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
    #[tracing::instrument(name = "get_product_events", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
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
            ProductListingEventReadError::ProductListingEventQueryFailed => {
                Self::ProductListingEventQueryFailed
            }
            ProductListingEventReadError::ProductListingEventReadModelInvalid => {
                Self::ProductListingEventReadModelInvalid
            }
        }
    }
}
