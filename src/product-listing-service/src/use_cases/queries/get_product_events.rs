use crate::ports::{ProductEventReadError, ProductEventReader, ProductEventReaderFactory};
use application::operation_context::OperationContext;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_listing_core::product_id::ProductId;
use product_listing_core::product_lifecycle::ProductLifecycle;
use product_listing_core::product_slug_id::ProductSlugId;
use product_listing_core::product_state::ProductState;
use shop_core::shop_slug_id::ShopSlugId;

use indexmap::IndexSet;
use product_listing_core::product::{
    ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation,
};
use product_listing_core::product_image::ProductImage;
use product_listing_core::{description::Description, title::Title};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductEventLookup {
    ById(ProductId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductEventsRequest {
    pub lookup: ProductEventLookup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEvent {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductEventType,
    pub payload: ProductEventPayload,
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductEventType {
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
pub enum ProductEventPayload {
    Created(ProductCreatedEventPayload),
    StateChanged(ProductStateChangedEventPayload),
    AddressChanged(ProductAddressChangedEventPayload),
    PriceChanged(ProductPriceChangedEventPayload),
    UrlChanged(ProductUrlChangedEventPayload),
    ImagesChanged(ProductImagesChangedEventPayload),
    AuctionChanged(ProductAuctionChangedEventPayload),
    Deleted(ProductDeletedEventPayload),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreatedEventPayload {
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub address: ProductAddress,
    pub pricing: ProductPricing,
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChangedEventPayload {
    pub old_state: ProductState,
    pub new_state: ProductState,
    pub sale_valuation: Option<ProductSaleValuation>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductAddressChangedEventPayload {
    pub address: ProductAddress,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChangedEventPayload {
    pub old_pricing: ProductPricing,
    pub new_pricing: ProductPricing,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductUrlChangedEventPayload {
    pub old_url: Url,
    pub new_url: Url,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChangedEventPayload {
    pub images: IndexSet<ProductImage>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionChangedEventPayload {
    pub auction: ProductAuction,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeletedEventPayload {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductEventsError {
    #[error("product not found")]
    NotFound,
    #[error("product event query failed")]
    ProductEventQueryFailed,
    #[error("product event read model is invalid")]
    ProductEventReadModelInvalid,
    #[error("failed to begin get product events transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product events transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductEventsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductEventsRequest,
    ) -> Result<Vec<ProductEvent>, GetProductEventsError>;
}

pub struct GetProductEventsHandler<U, R> {
    unit_of_work: U,
    reader: R,
}
impl<U, R> GetProductEventsHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetProductEventsUseCase for GetProductEventsHandler<U, R>
where
    U: UnitOfWork,
    R: ProductEventReaderFactory<U::Tx>,
{
    #[tracing::instrument(name = "get_product_events", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductEventsRequest,
    ) -> Result<Vec<ProductEvent>, GetProductEventsError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductEventsError::BeginTransactionFailed)?;
        let events = self
            .reader
            .in_transaction(&mut tx)
            .find_domain_events(&request.lookup)
            .await?
            .ok_or(GetProductEventsError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetProductEventsError::CommitTransactionFailed)?;
        Ok(events)
    }
}
impl From<ProductEventReadError> for GetProductEventsError {
    fn from(error: ProductEventReadError) -> Self {
        match error {
            ProductEventReadError::ProductEventQueryFailed => Self::ProductEventQueryFailed,
            ProductEventReadError::ProductEventReadModelInvalid => {
                Self::ProductEventReadModelInvalid
            }
        }
    }
}
