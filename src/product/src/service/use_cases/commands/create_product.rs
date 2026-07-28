use crate::core::description::Description;
use crate::core::product_aggregate::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing, RehydrateProductError,
};
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use crate::service::ports::product_event_store::ProductEventStoreError;
use crate::service::ports::product_repository::ProductRepositoryError;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::OperationContext;
use common::product_id::ProductId;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductCommand {
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub address: ProductAddress,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateProductResult {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateProductError {
    #[error("authenticated actor required to create product")]
    AuthenticatedActorRequired,
    #[error("product already exists for shop product key")]
    ProductKeyConflict,
    #[error("product slug already exists")]
    ProductSlugConflict,
    #[error("product state is invalid")]
    InvalidProductState,
    #[error("created product did not record a domain event")]
    CreatedEventMissing,
    #[error("product event already exists")]
    EventConflict,
    #[error("product repository unavailable")]
    ProductRepositoryUnavailable,
    #[error("product event store unavailable")]
    ProductEventStoreUnavailable,
    #[error("failed to begin create product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit create product transaction")]
    CommitTransactionFailed,
    #[error("internal product repository failure")]
    ProductRepositoryInternal,
    #[error("internal product event store failure")]
    ProductEventStoreInternal,
}

#[async_trait::async_trait]
pub trait CreateProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductCommand,
    ) -> Result<CreateProductResult, CreateProductError>;
}

impl CreateProductCommand {
    pub fn into_new_product(self, product_id: ProductId) -> NewProduct {
        NewProduct {
            id: product_id,
            shop_id: self.shop_id,
            seller_id: self.seller_id,
            shops_product_id: self.shops_product_id,
            address: self.address,
            title: self.title,
            description: self.description,
            pricing: self.pricing,
            state: self.state,
            url: self.url,
            images: self.images,
            auction: self.auction,
        }
    }
}

impl TryFrom<&Product> for CreateProductResult {
    type Error = CreateProductError;

    fn try_from(product: &Product) -> Result<Self, Self::Error> {
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductError::CreatedEventMissing)?;
        Ok(Self {
            product_id: product.id(),
            product_slug_id: product.slug_id().clone(),
            event_id,
        })
    }
}

impl From<RehydrateProductError> for CreateProductError {
    fn from(_error: RehydrateProductError) -> Self {
        Self::InvalidProductState
    }
}

impl From<ProductRepositoryError> for CreateProductError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ProductKeyConflict => Self::ProductKeyConflict,
            ProductRepositoryError::SlugConflict => Self::ProductSlugConflict,
            ProductRepositoryError::TemporarilyUnavailable => Self::ProductRepositoryUnavailable,
            ProductRepositoryError::InvalidPersistedState => Self::InvalidProductState,
            ProductRepositoryError::ConcurrencyConflict | ProductRepositoryError::Internal => {
                Self::ProductRepositoryInternal
            }
        }
    }
}

impl From<ProductEventStoreError> for CreateProductError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::EventConflict => Self::EventConflict,
            ProductEventStoreError::TemporarilyUnavailable => Self::ProductEventStoreUnavailable,
            ProductEventStoreError::InvalidEvent => Self::InvalidProductState,
            ProductEventStoreError::Internal => Self::ProductEventStoreInternal,
        }
    }
}
