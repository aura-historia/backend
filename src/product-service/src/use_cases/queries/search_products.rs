use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::OperationContext;
use common::pagination::cursor::{Cursor, CursoredResult};
use common::price::domain::Price;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::sort::Sort;
use indexmap::IndexSet;
use product_core::product_image::ProductImage;
use product_core::product_search::ProductSearch;
use product_core::sort_product_field::SortProductField;
use product_core::title::Title;
use serde_json::Value;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchProductsRequest {
    pub search: ProductSearch,
    pub sort: Option<Sort<SortProductField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductSummary {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub title: Option<Localized<Language, Title>>,
    pub price: Option<Price>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub updated: OffsetDateTime,
}

pub type SearchProductsResult = CursoredResult<ProductSummary, Value>;

#[derive(Debug, thiserror::Error)]
pub enum SearchProductsError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
}

#[async_trait::async_trait]
pub trait SearchProductsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchProductsRequest,
    ) -> Result<SearchProductsResult, SearchProductsError>;
}
