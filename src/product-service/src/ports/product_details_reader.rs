#![allow(dead_code)]

use crate::use_cases::queries::get_product::ProductLookup;
use crate::user_state::ProductUserState;
use common::event_id::EventId;
use common::personalized::Personalized;
use common::user_id::UserId;
use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing, ProductSaleValuation};
use product_core::product_id::ProductId;
use product_core::product_image::ProductImage;
use product_core::product_lifecycle::ProductLifecycle;
use product_core::product_slug_id::ProductSlugId;
use product_core::product_state::ProductState;
use product_core::shops_product_id::ShopsProductId;
use product_core::title::Title;
use shop_core::shop_id::ShopId;
use shop_core::shop_name::ShopName;
use shop_core::shop_slug_id::ShopSlugId;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsReadRequest {
    pub lookup: ProductLookup,
    pub language: Language,
    pub user_id: Option<UserId>,
}

/// Factual relational product detail. The use case owns currency presentation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsReadModel {
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
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub sale_valuation: Option<ProductSaleValuation>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

pub type PersonalizedProductDetailsReadModel =
    Personalized<ProductDetailsReadModel, ProductUserState>;

#[derive(Debug, thiserror::Error)]
pub enum ProductDetailsReadError {
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductDetailsReader: Send {
    async fn find_details(
        &mut self,
        request: &ProductDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductDetailsReadModel>, ProductDetailsReadError>;
}

pub trait ProductDetailsReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ProductDetailsReader + 'tx;
}
