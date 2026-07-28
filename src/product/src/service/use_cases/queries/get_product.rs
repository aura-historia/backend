use crate::core::description::Description;
use crate::core::product_aggregate::{ProductAddress, ProductAuction, ProductPricing};
use crate::core::product_image::ProductImage;
use crate::core::title::Title;
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::OperationContext;
use common::price::domain::Price;
use common::product_id::{ProductId, ProductKey};
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use indexmap::IndexSet;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum GetProductRequest {
    ById(ProductId),
    ByKey(ProductKey),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsView {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductAddress,
    pub native_title: Localized<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub currency: Currency,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductError {
    #[error("product not found")]
    NotFound,
    #[error("temporary product read failure")]
    TemporarilyUnavailable,
    #[error("invalid product read model")]
    InvalidReadModel,
    #[error("internal product read failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait GetProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<ProductDetailsView, GetProductError>;
}
