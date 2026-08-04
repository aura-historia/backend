use crate::ports::{ProductHistoryReadError, ProductHistoryReader, ProductHistoryReaderFactory};
use common::event_id::EventId;
use common::operation_context::OperationContext;
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_state::domain::ProductState;
use common::transaction::{Transaction, UnitOfWork};
use common::{language::domain::Language, localized::Localized};
use indexmap::IndexSet;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::{description::Description, title::Title};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductHistoryRequest {
    pub product_key: ProductKey,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductHistoryEvent {
    pub product_id: ProductId,
    pub event_id: EventId,
    pub event_type: ProductHistoryEventType,
    pub payload: ProductHistoryPayload,
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductHistoryEventType {
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
pub enum ProductHistoryPayload {
    Created(ProductCreatedHistoryPayload),
    StateChanged(ProductStateChangedHistoryPayload),
    AddressChanged(ProductAddressChangedHistoryPayload),
    PriceChanged(ProductPriceChangedHistoryPayload),
    UrlChanged(ProductUrlChangedHistoryPayload),
    ImagesChanged(ProductImagesChangedHistoryPayload),
    AuctionChanged(ProductAuctionChangedHistoryPayload),
    Deleted(ProductDeletedHistoryPayload),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductCreatedHistoryPayload {
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub address: ProductAddress,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductStateChangedHistoryPayload {
    pub old_state: ProductState,
    pub new_state: ProductState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAddressChangedHistoryPayload {
    pub address: ProductAddress,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductPriceChangedHistoryPayload {
    pub old_pricing: ProductPricing,
    pub new_pricing: ProductPricing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductUrlChangedHistoryPayload {
    pub old_url: Url,
    pub new_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductImagesChangedHistoryPayload {
    pub images: IndexSet<ProductImage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductAuctionChangedHistoryPayload {
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDeletedHistoryPayload {
    pub old_lifecycle: ProductLifecycle,
    pub new_lifecycle: ProductLifecycle,
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductHistoryError {
    #[error("product not found")]
    NotFound,
    #[error("product history query failed")]
    ProductHistoryQueryFailed,
    #[error("product history read model is invalid")]
    ProductHistoryReadModelInvalid,
    #[error("product history contains an unsupported event schema")]
    UnsupportedProductHistoryEventSchema,
    #[error("failed to begin get product history transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product history transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductHistoryUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductHistoryRequest,
    ) -> Result<Vec<ProductHistoryEvent>, GetProductHistoryError>;
}

pub struct GetProductHistoryHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetProductHistoryHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetProductHistoryUseCase for GetProductHistoryHandler<U, R>
where
    U: UnitOfWork,
    R: ProductHistoryReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_product_history",
        skip_all,
        fields(
            shop_id = %request.product_key.shop_id,
            shops_product_id = %request.product_key.shops_product_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductHistoryRequest,
    ) -> Result<Vec<ProductHistoryEvent>, GetProductHistoryError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductHistoryError::BeginTransactionFailed)?;
        let events = self
            .reader
            .in_transaction(&mut tx)
            .find_history(&request.product_key)
            .await?
            .ok_or(GetProductHistoryError::NotFound)?;
        tx.commit()
            .await
            .map_err(|_| GetProductHistoryError::CommitTransactionFailed)?;

        Ok(events)
    }
}

impl From<ProductHistoryReadError> for GetProductHistoryError {
    fn from(error: ProductHistoryReadError) -> Self {
        match error {
            ProductHistoryReadError::ProductHistoryQueryFailed => Self::ProductHistoryQueryFailed,
            ProductHistoryReadError::ProductHistoryReadModelInvalid => {
                Self::ProductHistoryReadModelInvalid
            }
            ProductHistoryReadError::UnsupportedProductHistoryEventSchema => {
                Self::UnsupportedProductHistoryEventSchema
            }
        }
    }
}
