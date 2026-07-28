use crate::core::description::Description;
use crate::core::product_aggregate::{
    NewProduct, Product, ProductAddress, ProductAuction, ProductPricing,
};
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
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
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub state: ProductState,
    pub url: Url,
    pub images: IndexSet<ProductImage>,
    pub embedding: Option<Vec<f32>>,
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
    #[error("product already exists")]
    ProductConflict,
    #[error("product slug already exists")]
    SlugConflict,
    #[error("operation not permitted")]
    Forbidden,
    #[error("invalid product state")]
    InvalidProduct,
    #[error("temporary persistence failure")]
    TemporarilyUnavailable,
    #[error("internal failure")]
    Internal,
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
            native_title: self.native_title,
            native_description: self.native_description,
            pricing: self.pricing,
            state: self.state,
            url: self.url,
            images: self.images,
            embedding: self.embedding,
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
            .ok_or(CreateProductError::Internal)?;
        Ok(Self {
            product_id: product.id(),
            product_slug_id: product.slug_id().clone(),
            event_id,
        })
    }
}
