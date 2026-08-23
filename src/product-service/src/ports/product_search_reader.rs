#![allow(dead_code)]

use crate::{
    ports::ProductPriceFilterPlan, use_cases::queries::search_products::ProductSearchReadResult,
};
use application::pagination::Cursor;
use domain_primitives::sort::Sort;
use product_core::{product_search::ProductSearch, sort_product_field::SortProductField};
use serde_json::Value;

/// A Product search compiled against one persisted FX snapshot.
///
/// The raw `ProductSearch` is retained for non-price filters. OpenSearch adapters must render
/// price clauses only from `price_filter_plan`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompiledProductSearch {
    pub search: ProductSearch,
    pub price_filter_plan: ProductPriceFilterPlan,
}

/// One adapter-facing Product search request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductSearchReadRequest {
    pub compiled_search: CompiledProductSearch,
    pub sort: Option<Sort<SortProductField>>,
    pub cursor: Option<Cursor<Value>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSearchReadError {
    #[error("product search query failed")]
    ProductSearchQueryFailed,
    #[error("product search read model is invalid")]
    ProductSearchReadModelInvalid,
}

#[async_trait::async_trait]
pub trait ProductSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &ProductSearchReadRequest,
    ) -> Result<ProductSearchReadResult, ProductSearchReadError>;

    async fn search_hybrid(
        &self,
        request: &ProductSearchReadRequest,
        embedding: &[f32],
    ) -> Result<ProductSearchReadResult, ProductSearchReadError>;
}
